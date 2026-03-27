//! MCP (Model Context Protocol) server for Kettu.
//!
//! Implements a lightweight JSON-RPC 2.0 server over stdio, exposing Kettu's
//! compiler and documentation as MCP tools for AI/LM integration.
//!
//! Protocol lifecycle:
//! 1. Client sends `initialize` → server returns capabilities
//! 2. Client sends `notifications/initialized` → ack
//! 3. Client sends `tools/list` → server returns tool definitions
//! 4. Client sends `tools/call` → server dispatches to handler

use crate::docs;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

/// MCP protocol version we implement.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Run the MCP server — reads JSON-RPC messages from stdin, writes responses to stdout.
pub fn run_server() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                let error_response = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32700,
                        "message": format!("Parse error: {}", e)
                    }
                });
                writeln!(stdout, "{}", error_response).ok();
                stdout.flush().ok();
                continue;
            }
        };

        let method = request["method"].as_str().unwrap_or("");
        let id = request.get("id").cloned();

        let response = match method {
            "initialize" => Some(handle_initialize(id)),
            "notifications/initialized" => None, // notification, no response
            "tools/list" => Some(handle_tools_list(id)),
            "tools/call" => Some(handle_tools_call(id, &request["params"])),
            _ => {
                if let Some(id_val) = id {
                    Some(json!({
                        "jsonrpc": "2.0",
                        "id": id_val,
                        "error": {
                            "code": -32601,
                            "message": format!("Method not found: {}", method)
                        }
                    }))
                } else {
                    None // unknown notification, ignore
                }
            }
        };

        if let Some(resp) = response {
            writeln!(stdout, "{}", resp).ok();
            stdout.flush().ok();
        }
    }
}

fn handle_initialize(id: Option<Value>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "kettu",
                "version": env!("CARGO_PKG_VERSION")
            }
        }
    })
}

fn handle_tools_list(id: Option<Value>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "tools": [
                {
                    "name": "check",
                    "description": "Type-check Kettu/WIT source code. Returns diagnostics (errors and warnings) or 'OK' if clean.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "source": {
                                "type": "string",
                                "description": "Kettu source code to check"
                            }
                        },
                        "required": ["source"]
                    }
                },
                {
                    "name": "parse",
                    "description": "Parse Kettu/WIT source code and return a summary of the AST structure (packages, interfaces, worlds, functions, types).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "source": {
                                "type": "string",
                                "description": "Kettu source code to parse"
                            }
                        },
                        "required": ["source"]
                    }
                },
                {
                    "name": "docs-search",
                    "description": "Search the Kettu language guide by keyword. Returns matching topics with selectors and snippets.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "Search query (e.g. 'lists', 'async', 'pattern matching')"
                            }
                        },
                        "required": ["query"]
                    }
                },
                {
                    "name": "docs-read",
                    "description": "Read a specific topic from the Kettu language guide. Use docs-search first to find topic numbers.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "topic": {
                                "type": "string",
                                "description": "Topic selector (e.g. '1.2' for Data Types, '2.1' for SIMD)"
                            }
                        },
                        "required": ["topic"]
                    }
                },
                {
                    "name": "emit-wit",
                    "description": "Strip Kettu extensions from source code and emit pure WIT (WebAssembly Interface Types).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "source": {
                                "type": "string",
                                "description": "Kettu source code to convert to WIT"
                            }
                        },
                        "required": ["source"]
                    }
                }
            ]
        }
    })
}

fn handle_tools_call(id: Option<Value>, params: &Value) -> Value {
    let tool_name = params["name"].as_str().unwrap_or("");
    let args = &params["arguments"];

    let result = match tool_name {
        "check" => tool_check(args),
        "parse" => tool_parse(args),
        "docs-search" => tool_docs_search(args),
        "docs-read" => tool_docs_read(args),
        "emit-wit" => tool_emit_wit(args),
        _ => Err(format!("Unknown tool: {}", tool_name)),
    };

    match result {
        Ok(text) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [{ "type": "text", "text": text }],
                "isError": false
            }
        }),
        Err(msg) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [{ "type": "text", "text": msg }],
                "isError": true
            }
        }),
    }
}

// ============================================================================
// Tool Handlers
// ============================================================================

fn tool_check(args: &Value) -> Result<String, String> {
    let source = args["source"]
        .as_str()
        .ok_or("Missing required argument: source")?;

    let (ast, parse_errors) = kettu_parser::parse_file(source);

    let mut messages = Vec::new();

    for e in &parse_errors {
        messages.push(format!("Parse error: {}", e));
    }

    if let Some(ast) = &ast {
        let diagnostics = kettu_checker::check(ast);
        for d in &diagnostics {
            let severity = match d.severity {
                kettu_checker::Severity::Error => "Error",
                kettu_checker::Severity::Warning => "Warning",
                kettu_checker::Severity::Info => "Info",
            };
            messages.push(format!("{}: {}", severity, d.message));
        }
    }

    if messages.is_empty() {
        Ok("OK — no errors or warnings.".to_string())
    } else {
        Ok(messages.join("\n"))
    }
}

fn tool_parse(args: &Value) -> Result<String, String> {
    let source = args["source"]
        .as_str()
        .ok_or("Missing required argument: source")?;

    let (ast, parse_errors) = kettu_parser::parse_file(source);

    let mut output = String::new();

    if !parse_errors.is_empty() {
        output.push_str("Parse errors:\n");
        for e in &parse_errors {
            output.push_str(&format!("  {}\n", e));
        }
    }

    if let Some(ast) = &ast {
        // Summarize top-level items
        if let Some(pkg) = &ast.package {
            let ns = pkg.path.namespace.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(":");
            let name = pkg.path.name.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join("/");
            output.push_str(&format!("Package: {}:{}\n", ns, name));
        }

        for item in &ast.items {
            match item {
                kettu_parser::TopLevelItem::Interface(iface) => {
                    output.push_str(&format!(
                        "Interface: {} ({} items)\n",
                        iface.name.name,
                        iface.items.len()
                    ));
                    for ii in &iface.items {
                        match ii {
                            kettu_parser::InterfaceItem::Func(f) => {
                                output.push_str(&format!("  func: {}\n", f.name.name));
                            }
                            kettu_parser::InterfaceItem::TypeDef(td) => {
                                let name = match &td.kind {
                                    kettu_parser::TypeDefKind::Record { name, .. } => &name.name,
                                    kettu_parser::TypeDefKind::Enum { name, .. } => &name.name,
                                    kettu_parser::TypeDefKind::Variant { name, .. } => &name.name,
                                    kettu_parser::TypeDefKind::Flags { name, .. } => &name.name,
                                    kettu_parser::TypeDefKind::Resource { name, .. } => &name.name,
                                    kettu_parser::TypeDefKind::Alias { name, .. } => &name.name,
                                };
                                output.push_str(&format!("  type: {}\n", name));
                            }
                            kettu_parser::InterfaceItem::Use(u) => {
                                output.push_str(&format!("  use: {}\n", u.path.interface.name));
                            }
                        }
                    }
                }
                kettu_parser::TopLevelItem::World(world) => {
                    output.push_str(&format!(
                        "World: {} ({} items)\n",
                        world.name.name,
                        world.items.len()
                    ));
                }
                kettu_parser::TopLevelItem::Use(u) => {
                    output.push_str(&format!("Use: {}\n", u.path.interface.name));
                }
                kettu_parser::TopLevelItem::NestedPackage(_) => {
                    output.push_str("NestedPackage\n");
                }
            }
        }
    }

    if output.is_empty() {
        Ok("No AST produced.".to_string())
    } else {
        Ok(output)
    }
}

fn tool_docs_search(args: &Value) -> Result<String, String> {
    let query = args["query"]
        .as_str()
        .ok_or("Missing required argument: query")?;

    let results = docs::search_docs_results(query);

    if results.is_empty() {
        return Ok(format!("No results found for \"{}\".", query));
    }

    let mut output = format!("Search results for \"{}\":\n\n", query);
    for (selector, title, snippet) in &results {
        output.push_str(&format!("  {}  {}\n", selector, title));
        if !snippet.is_empty() {
            output.push_str(&format!("      {}\n", snippet));
        }
    }
    output.push_str("\nUse the docs-read tool with a topic number to read the full content.");

    Ok(output)
}

fn tool_docs_read(args: &Value) -> Result<String, String> {
    let topic = args["topic"]
        .as_str()
        .ok_or("Missing required argument: topic")?;

    docs::get_topic_text(topic)
        .ok_or_else(|| format!("Topic '{}' not found. Use docs-search to find valid topic numbers.", topic))
}

fn tool_emit_wit(args: &Value) -> Result<String, String> {
    let source = args["source"]
        .as_str()
        .ok_or("Missing required argument: source")?;

    let (ast, errors) = kettu_parser::parse_file(source);

    if !errors.is_empty() {
        let mut msg = String::from("Parse errors:\n");
        for e in &errors {
            msg.push_str(&format!("  {}\n", e));
        }
        return Err(msg);
    }

    if let Some(ast) = ast {
        Ok(kettu_parser::emit_wit(&ast))
    } else {
        Err("No AST produced.".to_string())
    }
}
