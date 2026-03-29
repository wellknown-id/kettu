//! Kettu Parser
//!
//! A rust-sitter/Tree Sitter–based parser for the Kettu language, which is fully
//! compatible with WIT (WebAssembly Interface Types) and extends it with function bodies.

pub mod ast;
pub mod capture;
pub mod emitter;
pub mod grammar;
pub mod lexer;

pub use ast::*;
pub use emitter::emit_wit;

/// Error type for parse results
pub type ParseError = rust_sitter::error::ParseError;

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

/// Parse Kettu/WIT source code using the rust-sitter grammar.
///
/// Returns an optional AST (present even on partial parse with error recovery)
/// and a vector of parse errors.
pub fn parse(source: &str) -> (Option<ast::WitFile>, Vec<ParseError>) {
    use rust_sitter::Language;

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
    fn test_find_comment_ranges() {
        let source = "interface i { // single line comment\n /* block comment */ \n }";
        let ranges = find_comment_ranges(source);
        assert_eq!(ranges.len(), 2);

        let comment1 = &source[ranges[0].clone()];
        assert_eq!(comment1, "// single line comment");

        let comment2 = &source[ranges[1].clone()];
        assert_eq!(comment2, "/* block comment */");
    }

    #[test]
    fn test_find_comment_ranges_with_strings() {
        let source = r#"interface i { " // not a comment " }"#;
        let ranges = find_comment_ranges(source);
        assert_eq!(ranges.len(), 0);
    }

    #[test]
    fn test_strip_comments_preserve_layout() {
        let source = "let x = 1; // comment\nlet y = 2; /* block \n comment */ let z = 3;";
        let stripped = strip_comments_preserve_layout(source);

        // Should preserve newlines
        assert_eq!(
            stripped,
            "let x = 1;           \nlet y = 2;          \n            let z = 3;"
        );
        assert_eq!(source.len(), stripped.len());
    }

    #[test]
    fn test_strip_comments_empty() {
        let source = "let x = 1;";
        let stripped = strip_comments_preserve_layout(source);
        assert_eq!(source, stripped);
    }

    #[test]
    fn test_parse_valid() {
        let source = "interface i { f: func(); }";
        let (ast, errors) = parse(source);
        assert!(errors.is_empty());
        assert!(ast.is_some());

        let file = ast.unwrap();
        assert_eq!(file.items.len(), 1);
    }

    #[test]
    fn test_parse_invalid() {
        let source = "interface { f: func(); }"; // missing name
        let (_ast, errors) = parse(source);
        assert!(!errors.is_empty());
    }

    #[test]
    fn test_parse_file_delegation() {
        let source = "interface i { f: func(); }";
        let (ast1, _err1) = parse(source);
        let (ast2, _err2) = parse_file(source);

        assert_eq!(ast1, ast2);
    }
}
