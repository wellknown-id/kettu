//! Doc-testing: extract kettu code blocks from embedded docs and verify them.
//!
//! Supports three modes via the fenced code block info string:
//! - `check` (default) — parse + type-check the snippet
//! - `parse` — parse only (useful for expression-only demos)
//! - `nocheck` — skip entirely

/// Outcome mode for a code block.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Check,
    Parse,
    NoCheck,
}

/// A fenced code block extracted from a markdown doc.
#[derive(Debug)]
pub struct CodeBlock {
    /// 1-based line number in the source markdown.
    pub line: usize,
    /// The check mode.
    mode: Mode,
    /// Whether the block is standalone (has package/interface).
    standalone: bool,
    /// Raw code content.
    pub code: String,
}

/// Extract all kettu code blocks from a markdown source string.
pub fn extract_blocks(source: &str) -> Vec<CodeBlock> {
    let mut blocks = Vec::new();
    let lines: Vec<&str> = source.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();

        // Match ```kettu, ```kettu check, ```kettu nocheck, ```kettu parse, etc.
        if line.starts_with("```kettu") && !line.starts_with("````") {
            let info = &line[3..]; // "kettu ..." or "kettu"
            let mode = parse_mode(info);

            let start_line = i + 1; // 1-based
            let mut code_lines = Vec::new();
            i += 1;

            // Collect until closing ```
            while i < lines.len() {
                if lines[i].trim() == "```" {
                    break;
                }
                code_lines.push(lines[i]);
                i += 1;
            }

            let code = code_lines.join("\n");
            let standalone =
                code.contains("package ") || code.contains("interface ") || code.contains("world ");

            blocks.push(CodeBlock {
                line: start_line,
                mode,
                standalone,
                code,
            });
        }

        i += 1;
    }

    blocks
}

/// Parse the mode from a fenced code block info string like "kettu check" or "kettu nocheck".
fn parse_mode(info: &str) -> Mode {
    let parts: Vec<&str> = info.split_whitespace().collect();
    if parts.len() >= 2 {
        match parts[1] {
            "nocheck" => Mode::NoCheck,
            "parse" => Mode::Parse,
            "check" => Mode::Check,
            "standalone" => Mode::Check, // standalone implies check
            _ => Mode::Check,
        }
    } else {
        Mode::Check // default
    }
}

/// Wrap a partial snippet in a valid kettu file structure.
/// If `preamble` is provided, it's prepended inside the function body
/// to provide variable/function declarations needed by the snippet.
fn wrap_snippet(code: &str, preamble: Option<&str>) -> String {
    let trimmed = code.trim();
    let preamble_block = preamble.unwrap_or("");

    // Type-only: record, variant, enum, flags, type alias — put inside interface directly
    let is_type_decl = trimmed.starts_with("record ")
        || trimmed.starts_with("variant ")
        || trimmed.starts_with("enum ")
        || trimmed.starts_with("flags ")
        || (trimmed.starts_with("type ") && trimmed.contains('='));

    if is_type_decl {
        return format!(
            "package local:doctest;\n\ninterface snippet {{\n{}\n}}\n",
            indent(trimmed, 4)
        );
    }

    // Function signatures (no body) — put inside interface
    if is_func_signature(trimmed) {
        return format!(
            "package local:doctest;\n\ninterface snippet {{\n{}\n}}\n",
            indent(trimmed, 4)
        );
    }

    // Gate annotations (@since, @deprecated, @unstable, @test) followed by interface items
    if trimmed.starts_with('@') {
        return format!(
            "package local:doctest;\n\ninterface snippet {{\n{}\n}}\n",
            indent(trimmed, 4)
        );
    }

    // Constructor declarations
    if trimmed.starts_with("constructor(") {
        return format!(
            "package local:doctest;\n\ninterface snippet {{\n    resource r {{\n{}\n    }}\n}}\n",
            indent(trimmed, 8)
        );
    }

    // World declarations
    if trimmed.starts_with("world ") {
        return format!("package local:doctest;\n\n{}\n", trimmed);
    }

    // Statements — wrap in a function body, with optional preamble
    if preamble_block.is_empty() {
        format!(
            "package local:doctest;\n\ninterface snippet {{\n    run: func() {{\n{}\n    }}\n}}\n",
            indent(trimmed, 8)
        )
    } else {
        format!(
            "package local:doctest;\n\ninterface snippet {{\n    run: func() {{\n{}\n{}\n    }}\n}}\n",
            indent(preamble_block, 8),
            indent(trimmed, 8)
        )
    }
}

/// Check if code looks like bare function signature(s).
fn is_func_signature(code: &str) -> bool {
    // Accept if all non-blank, non-comment lines look like func sigs
    code.lines().all(|l| {
        let t = l.trim();
        t.is_empty()
            || t.starts_with("//")
            || t.contains(": func(")
            || t.contains(": async func(")
            || t.contains(": static func(")
    })
}

/// Indent every line of `text` by `n` spaces.
fn indent(text: &str, n: usize) -> String {
    let pad: String = " ".repeat(n);
    text.lines()
        .map(|l| {
            if l.trim().is_empty() {
                String::new()
            } else {
                format!("{}{}", pad, l)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Run doc-tests for a list of (title, content, preamble) pages.
/// Returns (passed, failed, skipped).
pub fn run_doctests(pages: &[(&str, &str, Option<&str>)]) -> (usize, usize, usize) {
    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;

    for (title, content, preamble) in pages {
        let blocks = extract_blocks(content);
        let checkable: Vec<&CodeBlock> =
            blocks.iter().filter(|b| b.mode != Mode::NoCheck).collect();

        if checkable.is_empty() {
            continue;
        }

        let mut file_passed = 0;
        let mut file_failed = 0;
        let mut file_skipped = 0;

        for block in &blocks {
            match block.mode {
                Mode::NoCheck => {
                    file_skipped += 1;
                    continue;
                }
                Mode::Parse => {
                    let source = if block.standalone {
                        block.code.clone()
                    } else {
                        wrap_snippet(&block.code, *preamble)
                    };

                    let (_ast, errors) = kettu_parser::parse_file(&source);
                    if errors.is_empty() {
                        file_passed += 1;
                    } else {
                        file_failed += 1;
                        eprintln!(
                            "  \x1b[31m\u{2717}\x1b[0m {} line {} (parse error)",
                            title, block.line
                        );
                        for e in &errors {
                            eprintln!("    {}", e);
                        }
                    }
                }
                Mode::Check => {
                    let source = if block.standalone {
                        block.code.clone()
                    } else {
                        wrap_snippet(&block.code, *preamble)
                    };

                    let (ast, parse_errors) = kettu_parser::parse_file(&source);
                    if !parse_errors.is_empty() {
                        file_failed += 1;
                        eprintln!(
                            "  \x1b[31m\u{2717}\x1b[0m {} line {} (parse error)",
                            title, block.line
                        );
                        for e in &parse_errors {
                            eprintln!("    {}", e);
                        }
                        continue;
                    }

                    if let Some(ast) = ast {
                        let diagnostics = kettu_checker::check(&ast);
                        let errors: Vec<_> = diagnostics
                            .iter()
                            .filter(|d| matches!(d.severity, kettu_checker::Severity::Error))
                            .collect();

                        if errors.is_empty() {
                            file_passed += 1;
                        } else {
                            file_failed += 1;
                            eprintln!(
                                "  \x1b[31m\u{2717}\x1b[0m {} line {} (check error)",
                                title, block.line
                            );
                            for e in &errors {
                                eprintln!("    {}", e.message);
                            }
                        }
                    } else {
                        file_failed += 1;
                        eprintln!(
                            "  \x1b[31m\u{2717}\x1b[0m {} line {} (no AST produced)",
                            title, block.line
                        );
                    }
                }
            }
        }

        if file_failed > 0 {
            eprintln!();
        }

        passed += file_passed;
        failed += file_failed;
        skipped += file_skipped;
    }

    (passed, failed, skipped)
}
