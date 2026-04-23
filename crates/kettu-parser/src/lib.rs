//! Kettu Parser
//!
//! A krust-sitter/Tree Sitter–based parser for the Kettu language, which is fully
//! compatible with WIT (WebAssembly Interface Types) and extends it with function bodies.

pub mod ast;
pub mod capture;
pub mod emitter;
pub mod grammar;
pub mod lexer;

pub use ast::*;
pub use emitter::emit_wit;

/// Error type for parse results
pub type ParseError = krust_sitter::error::ParseError;

fn find_comment_ranges(source: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut index = 0;

    while index < len {
        if index + 1 < len && bytes[index] == b'/' && bytes[index + 1] == b'/' {
            let start = index;
            index += 2;
            while index < len && bytes[index] != b'\n' {
                index += 1;
            }
            ranges.push(start..index);
        } else if index + 1 < len && bytes[index] == b'/' && bytes[index + 1] == b'*' {
            let start = index;
            index += 2;
            while index + 1 < len {
                if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                    index += 2;
                    break;
                }
                index += 1;
            }
            ranges.push(start..index);
        } else if bytes[index] == b'"' {
            index += 1;
            while index < len && bytes[index] != b'"' {
                if bytes[index] == b'\\' {
                    index += 1;
                }
                index += 1;
            }
            if index < len {
                index += 1;
            }
        } else {
            index += 1;
        }
    }

    ranges
}

fn strip_comments_preserve_layout(source: &str) -> String {
    let ranges = find_comment_ranges(source);
    if ranges.is_empty() {
        return source.to_string();
    }

    let mut bytes = source.as_bytes().to_vec();
    for range in ranges {
        for index in range {
            if bytes[index] != b'\n' {
                bytes[index] = b' ';
            }
        }
    }

    String::from_utf8(bytes).unwrap_or_else(|_| source.to_string())
}

/// Parse Kettu/WIT source code using the krust-sitter grammar.
///
/// Returns an optional AST (present even on partial parse with error recovery)
/// and a vector of parse errors.
pub fn parse(source: &str) -> (Option<ast::WitFile>, Vec<ParseError>) {
    use krust_sitter::Language;

    let normalized = strip_comments_preserve_layout(source);
    let result = grammar::WitFile::parse(&normalized);
    let errors = result.errors;

    let ast = result.result.map(|cst| grammar::convert::wit_file(&cst));
    (ast, errors)
}

/// Parse a Kettu/WIT source file and return the AST with any errors.
pub fn parse_file(source: &str) -> (Option<ast::WitFile>, Vec<ParseError>) {
    parse(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_kettu_code() {
        let source = "package test:pkg;\n\ninterface i {\n  f: func();\n}\n";
        let (ast, errors) = parse(source);
        assert!(ast.is_some(), "AST should be present for valid code");
        assert!(errors.is_empty(), "There should be no errors for valid code");
    }

    #[test]
    fn test_parse_invalid_kettu_code() {
        let source = "this is not valid kettu code";
        let (_ast, errors) = parse(source);
        // Even for invalid code, parser usually returns Some(ast) with error recovery
        assert!(!errors.is_empty(), "There should be errors for invalid code");
    }

    #[test]
    fn test_parse_with_comments() {
        let source = r#"
            // This is a single line comment
            package test:pkg; /* This is a
            multi-line comment */
            interface i {
                // Another comment
                f: func(); /* inline comment */
            }
        "#;
        let (ast, errors) = parse(source);
        assert!(ast.is_some(), "AST should be present for code with comments");
        assert!(errors.is_empty(), "There should be no errors for valid code with comments");
    }

    #[test]
    fn test_strip_comments_preserve_layout_internals() {
        // Test single line comments
        assert_eq!(
            strip_comments_preserve_layout("a // comment\nb"),
            "a           \nb"
        );

        // Test multi-line comments
        assert_eq!(
            strip_comments_preserve_layout("a /* multi\nline */ b"),
            "a         \n        b"
        );

        // Test string literals (comments inside strings should not be stripped)
        assert_eq!(
            strip_comments_preserve_layout(r#"let x = "// not a comment"; // real comment"#),
            r#"let x = "// not a comment";                "#
        );
        assert_eq!(
            strip_comments_preserve_layout(r#"let x = "/* not a comment */"; /* real comment */"#),
            r#"let x = "/* not a comment */";                   "#
        );

        // Test string literals with escaped quotes
        assert_eq!(
            strip_comments_preserve_layout(r#"let x = "\"// not a comment\""; // real comment"#),
            r#"let x = "\"// not a comment\"";                "#
        );
    }
}
