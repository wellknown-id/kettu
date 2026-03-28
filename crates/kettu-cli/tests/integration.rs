//! Integration tests for the full Kettu compilation pipeline

use serde_json::{Value, json};
use std::io::Write;
use std::process::Command;
use tempfile::NamedTempFile;
use wasmparser::{Parser, Payload};

fn kettu_cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kettu"))
}

fn run_mcp_request(request: Value) -> Value {
    let mut child = kettu_cmd()
        .args(["mcp"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("Failed to start kettu mcp");

    let stdin = child.stdin.as_mut().expect("mcp stdin");
    writeln!(stdin, "{}", request).expect("write MCP request");
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("Failed to read MCP output");
    assert!(
        output.status.success(),
        "kettu mcp should exit successfully"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .expect("MCP response line");
    serde_json::from_str(line).expect("valid MCP JSON response")
}

#[test]
fn test_parse_command() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "package local:test;").unwrap();
    writeln!(file, "interface api {{").unwrap();
    writeln!(file, "    greet: func(name: string) -> string;").unwrap();
    writeln!(file, "}}").unwrap();

    let output = kettu_cmd()
        .args(["parse", file.path().to_str().unwrap()])
        .output()
        .expect("Failed to run kettu parse");

    assert!(output.status.success(), "Parse command should succeed");
}

#[test]
fn test_check_command() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "package local:test;").unwrap();
    writeln!(file, "interface api {{").unwrap();
    writeln!(file, "    greet: func(name: string) -> string;").unwrap();
    writeln!(file, "}}").unwrap();

    let output = kettu_cmd()
        .args(["check", file.path().to_str().unwrap()])
        .output()
        .expect("Failed to run kettu check");

    assert!(output.status.success(), "Check command should succeed");
}

#[test]
fn test_emit_wit_command() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "package local:test;").unwrap();
    writeln!(file, "interface api {{").unwrap();
    writeln!(file, "    greet: func(name: string) -> string {{").unwrap();
    writeln!(file, "        format(name);").unwrap();
    writeln!(file, "    }}").unwrap();
    writeln!(file, "}}").unwrap();

    let output = kettu_cmd()
        .args(["emit-wit", file.path().to_str().unwrap()])
        .output()
        .expect("Failed to run kettu emit-wit");

    assert!(output.status.success(), "Emit-wit command should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("interface api"),
        "Output should contain interface"
    );
    assert!(
        stdout.contains("greet: func"),
        "Output should contain function"
    );
    // Should NOT contain function body
    assert!(
        !stdout.contains("format"),
        "Output should NOT contain function body"
    );
}

#[test]
fn test_build_core_command() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "package local:test;").unwrap();
    writeln!(file, "interface api {{").unwrap();
    writeln!(file, "    greet: func(x: s32) -> s32 {{").unwrap();
    writeln!(file, "        return x;").unwrap();
    writeln!(file, "    }}").unwrap();
    writeln!(file, "}}").unwrap();

    let output_file = NamedTempFile::new().unwrap();

    let output = kettu_cmd()
        .args([
            "build",
            "--core",
            file.path().to_str().unwrap(),
            "-o",
            output_file.path().to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run kettu build");

    assert!(
        output.status.success(),
        "Build command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Check output file exists and has valid WASM magic
    let wasm = std::fs::read(output_file.path()).unwrap();
    assert!(wasm.len() > 8, "Output should be non-empty WASM");
    assert_eq!(
        &wasm[0..4],
        b"\0asm",
        "Output should have WASM magic number"
    );
}

#[test]
fn test_build_with_expressions() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "package local:test;").unwrap();
    writeln!(file, "interface math {{").unwrap();
    writeln!(file, "    add: func(a: s32, b: s32) -> s32 {{").unwrap();
    writeln!(file, "        let sum = a + b;").unwrap();
    writeln!(file, "        return sum;").unwrap();
    writeln!(file, "    }}").unwrap();
    writeln!(file, "}}").unwrap();

    let output_file = NamedTempFile::new().unwrap();

    let output = kettu_cmd()
        .args([
            "build",
            "--core",
            file.path().to_str().unwrap(),
            "-o",
            output_file.path().to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run kettu build");

    assert!(
        output.status.success(),
        "Build with expressions should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let wasm = std::fs::read(output_file.path()).unwrap();
    assert!(wasm.len() > 8, "Output should be valid WASM");
    assert_eq!(
        &wasm[0..4],
        b"\0asm",
        "Output should have WASM magic number"
    );
}

#[test]
fn test_build_debug_sections() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "package local:test;").unwrap();
    writeln!(file, "interface api {{").unwrap();
    writeln!(file, "    add: func(a: s32, b: s32) -> s32 {{").unwrap();
    writeln!(file, "        return a + b;").unwrap();
    writeln!(file, "    }}").unwrap();
    writeln!(file, "}}").unwrap();

    let output_file = NamedTempFile::new().unwrap();

    let output = kettu_cmd()
        .args([
            "build",
            "--core",
            "--debug",
            file.path().to_str().unwrap(),
            "-o",
            output_file.path().to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run kettu build --debug");

    assert!(
        output.status.success(),
        "Debug build should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let wasm = std::fs::read(output_file.path()).unwrap();
    let mut has_debug_info = false;
    let mut has_debug_line = false;
    let mut has_name = false;

    for payload in Parser::new(0).parse_all(&wasm) {
        match payload.expect("valid wasm payload") {
            Payload::CustomSection(section) => {
                if section.name() == ".debug_info" {
                    has_debug_info = true;
                } else if section.name() == ".debug_line" {
                    has_debug_line = true;
                } else if section.name() == "name" {
                    has_name = true;
                }
            }
            _ => {}
        }
    }

    assert!(has_debug_info, "should emit .debug_info section");
    assert!(has_debug_line, "should emit .debug_line section");
    assert!(has_name, "should emit name section for debugging");
}

#[test]
fn test_build_debug_sections_include_lambda_locations() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "package local:test;").unwrap();
    writeln!(file, "interface tests {{").unwrap();
    writeln!(file, "    @test").unwrap();
    writeln!(file, "    closure: func() -> bool {{").unwrap();
    writeln!(file, "        let add-one = |x|").unwrap();
    writeln!(file, "            x + 1;").unwrap();
    writeln!(file, "        return add-one(1) == 2;").unwrap();
    writeln!(file, "    }}").unwrap();
    writeln!(file, "}}").unwrap();

    let output_file = NamedTempFile::new().unwrap();

    let output = kettu_cmd()
        .args([
            "build",
            "--core",
            "--debug",
            file.path().to_str().unwrap(),
            "-o",
            output_file.path().to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run kettu build --debug");

    assert!(
        output.status.success(),
        "Debug build should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let wasm = std::fs::read(output_file.path()).unwrap();
    let debug_info = Parser::new(0)
        .parse_all(&wasm)
        .find_map(|payload| match payload.expect("valid wasm payload") {
            Payload::CustomSection(section) if section.name() == ".debug_info" => {
                Some(String::from_utf8(section.data().to_vec()).expect("utf8 debug info"))
            }
            _ => None,
        })
        .expect("should emit debug info payload");

    assert!(
        debug_info.starts_with("kettu-dwarf-v2\n"),
        "expected the v2 debug payload format"
    );

    let lambda_entry = debug_info
        .lines()
        .find(|line| line.contains("\tlambda#0\t"))
        .expect("expected a lambda debug symbol");
    let parts: Vec<_> = lambda_entry.split('\t').collect();
    assert_eq!(parts[2], "5", "lambda start line should match source");
    assert_eq!(parts[3], "6", "lambda end line should match source");
}

#[test]
fn test_test_list_json() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "package local:test;").unwrap();
    writeln!(file, "interface tests {{").unwrap();
    writeln!(file, "    @test").unwrap();
    writeln!(file, "    smoke: func() -> bool {{ return true; }}").unwrap();
    writeln!(file, "    @test").unwrap();
    writeln!(file, "    smoke-extra: func() -> bool {{ return true; }}").unwrap();
    writeln!(file, "}}").unwrap();

    let output = kettu_cmd()
        .args(["test", file.path().to_str().unwrap(), "--list", "--json"])
        .output()
        .expect("Failed to run kettu test --list --json");

    assert!(
        output.status.success(),
        "List tests should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: Value = serde_json::from_str(&stdout).expect("valid json output");
    let tests = parsed["tests"].as_array().expect("tests should be array");
    assert_eq!(tests.len(), 2, "should list two tests");
    for test in tests {
        let line = test["line"].as_u64().expect("line should be number");
        let end_line = test["endLine"].as_u64().expect("endLine should be number");
        assert!(end_line >= line, "endLine should be >= line");
    }
}

#[test]
fn test_test_exact_filter_runs_one() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "package local:test;").unwrap();
    writeln!(file, "interface tests {{").unwrap();
    writeln!(file, "    @test").unwrap();
    writeln!(file, "    smoke: func() -> bool {{ return true; }}").unwrap();
    writeln!(file, "    @test").unwrap();
    writeln!(file, "    smoke-extra: func() -> bool {{ return true; }}").unwrap();
    writeln!(file, "}}").unwrap();

    let output = kettu_cmd()
        .args([
            "test",
            file.path().to_str().unwrap(),
            "--filter",
            "smoke",
            "--exact",
        ])
        .output()
        .expect("Failed to run kettu test --exact");

    assert!(
        output.status.success(),
        "Exact test run should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Running 1 test(s)"),
        "Exact filter should run one test: {}",
        stdout
    );
}

#[test]
fn test_check_type_error() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "package local:test;").unwrap();
    writeln!(file, "interface broken {{").unwrap();
    writeln!(file, "    bad: func(x: s32) -> s32 {{").unwrap();
    writeln!(file, "        return undefined_var;").unwrap();
    writeln!(file, "    }}").unwrap();
    writeln!(file, "}}").unwrap();

    let output = kettu_cmd()
        .args(["check", file.path().to_str().unwrap()])
        .output()
        .expect("Failed to run kettu check");

    // The check command should report an error for undefined variable
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}{}", stdout, stderr);

    // Check should detect the unknown variable
    assert!(
        combined.contains("Unknown variable")
            || combined.contains("undefined_var")
            || combined.contains("error"),
        "Check should report unknown variable error: {}",
        combined
    );
}

#[test]
fn test_docs_command() {
    let output = kettu_cmd()
        .args(["docs"])
        .output()
        .expect("Failed to run kettu docs");

    assert!(output.status.success(), "Docs command should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Language Topics"),
        "Output should contain 'Language Topics' section"
    );
    assert!(
        stdout.contains("Advanced Topics"),
        "Output should contain 'Advanced Topics' section"
    );
    assert!(
        stdout.contains("1.1"),
        "Output should contain numbered sub-topics"
    );
}

#[test]
fn test_docs_topic_command() {
    let output = kettu_cmd()
        .args(["docs", "1.1"])
        .output()
        .expect("Failed to run kettu docs 1.1");

    assert!(output.status.success(), "Docs topic command should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Hello World"),
        "Output should contain the Hello World topic content"
    );
}

#[test]
fn test_docs_check_command() {
    let output = kettu_cmd()
        .args(["docs", "--check"])
        .output()
        .expect("Failed to run kettu docs --check");

    assert!(
        output.status.success(),
        "Doc-tests should all pass: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("passed"),
        "Output should contain test results"
    );
    assert!(
        !stdout.contains("failed, ") || stdout.contains("0 failed"),
        "No doc-tests should fail"
    );
}

#[test]
fn test_docs_search_command() {
    let output = kettu_cmd()
        .args(["docs", "search", "lists"])
        .output()
        .expect("Failed to run kettu docs search");

    assert!(
        output.status.success(),
        "Search should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Lists"),
        "Search for 'lists' should find Lists topic"
    );
    assert!(
        stdout.contains("Search results"),
        "Output should contain search header"
    );
}

#[test]
fn test_mcp_initialize() {
    let response = run_mcp_request(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test" }
        }
    }));

    assert!(
        response.pointer("/result/protocolVersion") == Some(&json!("2024-11-05")),
        "Should return protocolVersion: {}",
        response
    );
    assert!(
        response.pointer("/result/capabilities/tools").is_some(),
        "Should advertise tools capability: {}",
        response
    );
}

#[test]
fn test_mcp_tools_call_check() {
    let response = run_mcp_request(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "check",
            "arguments": {
                "source": "interface math { add: func(a: s32, b: s32) -> s32 { a + b } }"
            }
        }
    }));

    assert!(
        response.pointer("/result/content/0/text") == Some(&json!("OK — no errors or warnings.")),
        "Valid code should pass check: {}",
        response
    );
    assert!(
        response.pointer("/result/isError") == Some(&json!(false)),
        "Should not be an error: {}",
        response
    );
}

#[test]
fn test_mcp_tools_list_includes_parse() {
    let response = run_mcp_request(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    }));

    let tools = response
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .expect("tools array");
    let names: Vec<_> = tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect();

    assert_eq!(names.len(), 5, "expected all advertised MCP tools");
    assert!(
        names.contains(&"parse"),
        "parse tool should be listed: {:?}",
        names
    );
}

#[test]
fn test_mcp_tools_call_parse() {
    let response = run_mcp_request(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "parse",
            "arguments": {
                "source": "package local:test; interface api { greet: func(name: string) -> string; }"
            }
        }
    }));

    let text = response
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .expect("parse tool text");

    assert!(
        text.contains("Package: local:test"),
        "should summarize the package: {}",
        text
    );
    assert!(
        text.contains("Interface: api"),
        "should summarize the interface: {}",
        text
    );
    assert!(
        text.contains("func: greet"),
        "should summarize functions: {}",
        text
    );
    assert_eq!(response.pointer("/result/isError"), Some(&json!(false)));
}
