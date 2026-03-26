//! Tests for the rust-sitter grammar, CST-to-AST conversion, span
//! information, and error recovery behaviour.

#[cfg(test)]
mod tests {
    use crate::parse;

    // ================================================================
    // Span verification tests
    // ================================================================

    #[test]
    fn test_interface_span_covers_full_definition() {
        let src = "interface my-iface {\n  greet: func(name: string) -> string;\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        assert_eq!(ast.items.len(), 1);
        match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(iface) => {
                assert_eq!(iface.name.name, "my-iface");
                // Span should start at 0 and end at the closing brace
                assert_eq!(iface.span.start, 0);
                assert_eq!(iface.span.end, src.len());
                // Name span should cover just "my-iface"
                let name_text = &src[iface.name.span.clone()];
                assert_eq!(name_text, "my-iface");
            }
            _ => panic!("expected Interface"),
        }
    }

    #[test]
    fn test_func_name_span() {
        let src = "interface i {\n  hello: func();\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!("expected Interface"),
        };
        let func = match &iface.items[0] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!("expected Func"),
        };
        assert_eq!(func.name.name, "hello");
        let name_text = &src[func.name.span.clone()];
        assert_eq!(name_text, "hello");
    }

    #[test]
    fn test_expression_spans_integer() {
        let src = "interface i {\n  f: func() {\n    42;\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!(),
        };
        let func = match &iface.items[0] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!(),
        };
        let body = func.body.as_ref().expect("should have body");
        let stmt = &body.statements[0];
        match stmt {
            crate::ast::Statement::Expr(e) => match e {
                crate::ast::Expr::Integer(val, span) => {
                    assert_eq!(*val, 42);
                    let text = &src[span.clone()];
                    assert_eq!(text, "42");
                }
                _ => panic!("expected Integer, got: {:?}", e),
            },
            _ => panic!("expected Expr statement"),
        }
    }

    #[test]
    fn test_expression_spans_binary_op() {
        let src = "interface i {\n  f: func() {\n    1 + 2;\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!(),
        };
        let func = match &iface.items[0] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!(),
        };
        let body = func.body.as_ref().unwrap();
        let stmt = &body.statements[0];
        match stmt {
            crate::ast::Statement::Expr(e) => match e {
                crate::ast::Expr::Binary { lhs, op, rhs, span } => {
                    assert_eq!(*op, crate::ast::BinOp::Add);
                    // The outer span covers "1 + 2"
                    let text = &src[span.clone()];
                    assert_eq!(text, "1 + 2");
                    // LHS should be "1"
                    match lhs.as_ref() {
                        crate::ast::Expr::Integer(1, lhs_span) => {
                            assert_eq!(&src[lhs_span.clone()], "1");
                        }
                        _ => panic!("expected Integer(1)"),
                    }
                    // RHS should be "2"
                    match rhs.as_ref() {
                        crate::ast::Expr::Integer(2, rhs_span) => {
                            assert_eq!(&src[rhs_span.clone()], "2");
                        }
                        _ => panic!("expected Integer(2)"),
                    }
                }
                _ => panic!("expected Binary"),
            },
            _ => panic!("expected Expr statement"),
        }
    }

    #[test]
    fn test_world_span() {
        let src = "world my-world {\n  export run: func();\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        match &ast.items[0] {
            crate::ast::TopLevelItem::World(w) => {
                assert_eq!(w.name.name, "my-world");
                assert_eq!(w.span.start, 0);
                assert_eq!(w.span.end, src.len());
                let name_text = &src[w.name.span.clone()];
                assert_eq!(name_text, "my-world");
            }
            _ => panic!("expected World"),
        }
    }

    #[test]
    fn test_typedef_span() {
        let src = "interface i {\n  record point {\n    x: u32,\n    y: u32\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!(),
        };
        match &iface.items[0] {
            crate::ast::InterfaceItem::TypeDef(td) => {
                // TypeDef span should be non-trivial (not 0..0)
                assert!(
                    td.span.start < td.span.end,
                    "typedef span should be non-trivial"
                );
                let text = &src[td.span.clone()];
                assert!(
                    text.contains("record point"),
                    "span text should contain the typedef"
                );
            }
            _ => panic!("expected TypeDef"),
        }
    }

    #[test]
    fn test_param_type_span() {
        let src = "interface i {\n  greet: func(name: string) -> u32;\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!(),
        };
        let func = match &iface.items[0] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!(),
        };
        // Check param type span
        let param = &func.params[0];
        assert_eq!(param.name.name, "name");
        match &param.ty {
            crate::ast::Ty::Primitive(crate::ast::PrimitiveTy::String, ty_span) => {
                assert!(
                    ty_span.start < ty_span.end,
                    "type span should be non-trivial"
                );
                assert_eq!(&src[ty_span.clone()], "string");
            }
            _ => panic!("expected string type"),
        }
        // Check return type span
        let ret = func.result.as_ref().expect("should have result");
        match ret {
            crate::ast::Ty::Primitive(crate::ast::PrimitiveTy::U32, ty_span) => {
                assert!(
                    ty_span.start < ty_span.end,
                    "return type span should be non-trivial"
                );
                assert_eq!(&src[ty_span.clone()], "u32");
            }
            _ => panic!("expected u32 return type"),
        }
    }

    #[test]
    fn test_package_decl_span() {
        let src = "package my:pkg;\n\ninterface i {}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        let pkg = ast.package.as_ref().expect("should have package");
        assert!(
            pkg.span.start < pkg.span.end,
            "package span should be non-trivial"
        );
        let text = &src[pkg.span.clone()];
        assert!(text.contains("package"));
    }

    // ================================================================
    // Error recovery tests
    // ================================================================

    #[test]
    fn test_error_recovery_missing_semicolon() {
        // Missing semicolons may or may not produce errors depending on error recovery
        let src = "interface i {\n  greet: func()\n}";
        let (ast, _errors) = parse(src);
        // Tree-sitter may or may not recover — just assert we get SOME result
        // (either an AST or errors, not both empty)
        assert!(
            ast.is_some() || !_errors.is_empty(),
            "should produce either AST or errors"
        );
    }

    #[test]
    fn test_error_recovery_invalid_token() {
        let src = "interface i {\n  @invalid: func();\n}";
        let (_ast, errors) = parse(src);
        assert!(!errors.is_empty(), "should have errors for invalid token");
    }

    #[test]
    fn test_error_recovery_truncated_input() {
        let src = "interface i {";
        let (_ast, errors) = parse(src);
        assert!(!errors.is_empty(), "should have errors for truncated input");
    }

    #[test]
    fn test_error_recovery_empty_input() {
        let src = "";
        let (ast, _errors) = parse(src);
        // Empty input may or may not produce a valid AST depending on
        // Tree-sitter's handling. Just verify we don't panic.
        if let Some(ast) = ast {
            assert!(ast.items.is_empty());
        }
    }

    #[test]
    fn test_error_recovery_gibberish() {
        let src = "asdf !@# $%^ 123";
        let (_, errors) = parse(src);
        assert!(!errors.is_empty(), "gibberish should produce errors");
    }

    #[test]
    fn test_error_position_is_meaningful() {
        let src = "interface i {\n  bad syntax here;\n}";
        let (_ast, errors) = parse(src);
        assert!(!errors.is_empty(), "should have errors");
        // Check that the error position is within the source
        for e in &errors {
            let start = e.error_position.bytes.start;
            let end = e.error_position.bytes.end;
            assert!(start <= src.len(), "error start should be within source");
            assert!(end <= src.len(), "error end should be within source");
        }
    }

    // ================================================================
    // Regression tests
    // ================================================================

    #[test]
    fn test_let_statement_value_span() {
        let src = "interface i {\n  f: func() {\n    let x = 123;\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!(),
        };
        let func = match &iface.items[0] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!(),
        };
        let body = func.body.as_ref().unwrap();
        match &body.statements[0] {
            crate::ast::Statement::Let { name, value } => {
                assert_eq!(name.name, "x");
                match value {
                    crate::ast::Expr::Integer(123, span) => {
                        assert_eq!(&src[span.clone()], "123");
                    }
                    _ => panic!("expected Integer(123)"),
                }
            }
            _ => panic!("expected Let"),
        }
    }

    #[test]
    fn test_trailing_commas_in_type_definitions() {
        let src = "interface i {\n  record request {\n    method: s32,\n    path: s32,\n  }\n\n  enum level {\n    debug,\n    info,\n  }\n\n  f: func(a: s32, b: s32,) -> tuple<s32, s32,>;\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        assert!(ast.is_some(), "should parse with trailing commas");
    }

    #[test]
    fn test_trailing_commas_in_expression_lists() {
        let src = "interface i {\n  f: func() {\n    g(1, 2,);\n    [1, 2, 3,];\n    { x: 1, y: 2, };\n    map([1, 2,], |x,| x);\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        assert!(ast.is_some(), "should parse with trailing commas");
    }

    #[test]
    fn test_assignment_and_slice_parse() {
        let src = "interface i {\n  f: func() {\n    let arr = [1, 2, 3, 4, 5];\n    let sum = 0;\n    for item in arr {\n      sum = sum + item;\n    };\n    let sub = arr[1..4];\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");

        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!("expected Interface"),
        };
        let func = match &iface.items[0] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!("expected Func"),
        };
        let body = func.body.as_ref().expect("should have body");

        let for_stmt = &body.statements[2];
        match for_stmt {
            crate::ast::Statement::Expr(crate::ast::Expr::ForEach { body, .. }) => {
                assert!(matches!(
                    body.first(),
                    Some(crate::ast::Statement::Assign { .. })
                ));
            }
            _ => panic!("expected for-each expression statement"),
        }

        match &body.statements[3] {
            crate::ast::Statement::Let { value, .. } => {
                assert!(matches!(value, crate::ast::Expr::Slice { .. }));
            }
            _ => panic!("expected let statement with slice value"),
        }
    }

    #[test]
    fn test_trailing_closure_calls_parse() {
        let src = "interface i {\n  f: func() {\n    let arr = [1, 2, 3];\n    let a = map(arr) |x| x * 2;\n    let b = apply |x| x * 2;\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");

        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!("expected Interface"),
        };
        let func = match &iface.items[0] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!("expected Func"),
        };
        let body = func.body.as_ref().expect("should have body");

        match &body.statements[1] {
            crate::ast::Statement::Let { value, .. } => match value {
                crate::ast::Expr::Map { .. } => {}
                _ => panic!("expected map builtin for map(arr) trailing closure form"),
            },
            _ => panic!("expected let statement"),
        }

        match &body.statements[2] {
            crate::ast::Statement::Let { value, .. } => match value {
                crate::ast::Expr::Call { args, .. } => assert_eq!(args.len(), 1),
                _ => panic!("expected call for apply trailing closure form"),
            },
            _ => panic!("expected let statement"),
        }
    }

    #[test]
    fn test_wit_package_qualified_paths_parse() {
        let src = "package wasi:http@0.3.0-rc-2026-02-09;\n\nuse wasi:io/streams@0.2.10 as streams;\n\ninterface handler {\n  use wasi:http/types@0.3.0-rc-2026-02-09.{request, response, error-code};\n}\n\nworld service {\n  include wasi:clocks/imports@0.2.10;\n  import wasi:cli/stdout@0.3.0-rc-2026-02-09;\n  export handler;\n}";

        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        assert!(ast.package.is_some(), "should parse versioned package");
        assert!(
            matches!(ast.items.first(), Some(crate::ast::TopLevelItem::Use(_))),
            "should parse top-level use"
        );
    }

    #[test]
    fn test_qualified_variant_literal_with_payload_parse() {
        let src = "interface i {\n  variant my-result { ok(s32), err(string), }\n  f: func() {\n    let r = my-result#ok(42);\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");

        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!("expected Interface"),
        };
        let func = match &iface.items[1] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!("expected Func"),
        };
        let body = func.body.as_ref().expect("should have body");

        match &body.statements[0] {
            crate::ast::Statement::Let { value, .. } => match value {
                crate::ast::Expr::VariantLiteral {
                    type_name,
                    case_name,
                    payload,
                    ..
                } => {
                    assert_eq!(
                        type_name.as_ref().map(|t| t.name.as_str()),
                        Some("my-result")
                    );
                    assert_eq!(case_name.name, "ok");
                    assert!(matches!(
                        payload.as_deref(),
                        Some(crate::ast::Expr::Integer(42, _))
                    ));
                }
                _ => panic!("expected qualified variant literal"),
            },
            _ => panic!("expected let statement"),
        }
    }

    #[test]
    fn test_qualified_variant_pattern_parse() {
        let src = "interface i {\n  variant my-result { ok(s32), err(string), }\n  f: func() -> bool {\n    let r = my-result#ok(1);\n    return match r {\n      my-result#ok(v) => v == 1,\n      my-result#err(_) => false,\n    };\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");

        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!("expected Interface"),
        };
        let func = match &iface.items[1] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!("expected Func"),
        };
        let body = func.body.as_ref().expect("should have body");

        let ret_expr = match &body.statements[1] {
            crate::ast::Statement::Return(Some(expr)) => expr,
            _ => panic!("expected return expression"),
        };

        match ret_expr {
            crate::ast::Expr::Match { arms, .. } => {
                let first = &arms[0].pattern;
                match first {
                    crate::ast::Pattern::Variant {
                        type_name,
                        case_name,
                        binding,
                        ..
                    } => {
                        assert_eq!(
                            type_name.as_ref().map(|t| t.name.as_str()),
                            Some("my-result")
                        );
                        assert_eq!(case_name.name, "ok");
                        assert_eq!(binding.as_ref().map(|b| b.name.as_str()), Some("v"));
                    }
                    _ => panic!("expected qualified variant pattern"),
                }
            }
            _ => panic!("expected match expression"),
        }
    }

    #[test]
    fn test_try_and_optional_chain_parse() {
        let src = "interface i {\n  record point { x: s32, }\n  f: func(p: option<point>) {\n    let v = p?.x;\n    let n = v?;\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");

        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!("expected Interface"),
        };
        let func = match &iface.items[1] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!("expected Func"),
        };
        let body = func.body.as_ref().expect("should have body");

        match &body.statements[0] {
            crate::ast::Statement::Let { value, .. } => match value {
                crate::ast::Expr::OptionalChain { field, .. } => {
                    assert_eq!(field.name, "x");
                }
                _ => panic!("expected optional chain expression"),
            },
            _ => panic!("expected let statement"),
        }

        match &body.statements[1] {
            crate::ast::Statement::Let { value, .. } => match value {
                crate::ast::Expr::Try { .. } => {}
                _ => panic!("expected try expression"),
            },
            _ => panic!("expected let statement"),
        }
    }

    #[test]
    fn test_malformed_qualified_variant_literal_missing_case_parse_error() {
        let src = "interface i {\n  f: func() {\n    let r = my-result#(42);\n  }\n}";
        let (_ast, errors) = parse(src);
        assert!(
            !errors.is_empty(),
            "expected parse errors for malformed qualified variant literal"
        );
    }

    #[test]
    fn test_malformed_qualified_variant_pattern_missing_case_parse_error() {
        let src = "interface i {\n  f: func() -> bool {\n    let r = #ok;\n    return match r {\n      my-result# => true,\n      _ => false,\n    };\n  }\n}";
        let (_ast, errors) = parse(src);
        assert!(
            !errors.is_empty(),
            "expected parse errors for malformed qualified variant pattern"
        );
    }

    #[test]
    fn test_string_literal_span() {
        let src = "interface i {\n  f: func() {\n    \"hello\";\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!(),
        };
        let func = match &iface.items[0] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!(),
        };
        let body = func.body.as_ref().unwrap();
        match &body.statements[0] {
            crate::ast::Statement::Expr(e) => match e {
                crate::ast::Expr::String(s, span) => {
                    assert_eq!(s, "hello");
                    // The span should cover the quotes too (the token includes them)
                    let text = &src[span.clone()];
                    assert_eq!(text, "\"hello\"");
                }
                _ => panic!("expected String"),
            },
            _ => panic!("expected Expr statement"),
        }
    }

    #[test]
    fn test_comment_tree_sitter_sexp() {
        // Parse directly with tree-sitter to see the raw S-expression
        let src = "package local:test;\n\n// Regular comment\n/// Doc comment\ninterface my-iface {\n    greet: func(name: string) -> string;\n}\n";
        let mut parser = rust_sitter::tree_sitter::Parser::new();
        parser
            .set_language(&crate::grammar::WitFile::language())
            .unwrap();
        let tree = parser.parse(src, None).unwrap();
        let root = tree.root_node();
        eprintln!("S-expression:\n{}", root.to_sexp());
        eprintln!("has_error: {}", root.has_error());

        // Walk the tree and print any ERROR or MISSING nodes
        fn walk_errors(node: rust_sitter::tree_sitter::Node, src: &str, depth: usize) {
            if node.is_error() {
                let text = &src[node.byte_range()];
                eprintln!(
                    "{}ERROR at {:?}: {:?}",
                    "  ".repeat(depth),
                    node.range(),
                    text
                );
            }
            if node.is_missing() {
                eprintln!(
                    "{}MISSING at {:?}: kind={}",
                    "  ".repeat(depth),
                    node.range(),
                    node.kind()
                );
            }
            if node.is_extra() {
                let text = &src[node.byte_range()];
                eprintln!(
                    "{}EXTRA at {:?}: kind={}, text={:?}",
                    "  ".repeat(depth),
                    node.range(),
                    node.kind(),
                    text
                );
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk_errors(child, src, depth + 1);
            }
        }
        walk_errors(root, src, 0);

        // Also test with rust-sitter parse
        use rust_sitter::Language;
        let result = crate::grammar::WitFile::parse(src);
        eprintln!("Parse errors: {:?}", result.errors);
        eprintln!("Parse result is_some: {}", result.result.is_some());
    }

    // ================================================================
    // Phase 13e: Ergonomic atomics parsing
    // ================================================================

    #[test]
    fn test_shared_let_statement_parse() {
        let src = "interface i {\n  f: func() {\n    shared let counter = 0;\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!("expected Interface"),
        };
        let func = match &iface.items[0] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!("expected Func"),
        };
        let body = func.body.as_ref().expect("should have body");
        match &body.statements[0] {
            crate::ast::Statement::SharedLet { name, initial_value } => {
                assert_eq!(name.name, "counter");
                assert!(matches!(initial_value, crate::ast::Expr::Integer(0, _)));
            }
            other => panic!("expected SharedLet, got: {:?}", other),
        }
    }

    #[test]
    fn test_atomic_block_expression_parse() {
        let src = "interface i {\n  f: func() {\n    atomic {\n      42;\n    };\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!("expected Interface"),
        };
        let func = match &iface.items[0] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!("expected Func"),
        };
        let body = func.body.as_ref().expect("should have body");
        match &body.statements[0] {
            crate::ast::Statement::Expr(crate::ast::Expr::AtomicBlock { body, .. }) => {
                assert!(!body.is_empty(), "atomic block should have at least one statement");
            }
            other => panic!("expected AtomicBlock expression, got: {:?}", other),
        }
    }

    #[test]
    fn test_shared_let_complex_initializer() {
        let src = "interface i {\n  f: func() {\n    shared let x = 1 + 2;\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!(),
        };
        let func = match &iface.items[0] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!(),
        };
        let body = func.body.as_ref().unwrap();
        match &body.statements[0] {
            crate::ast::Statement::SharedLet { name, initial_value } => {
                assert_eq!(name.name, "x");
                assert!(matches!(initial_value, crate::ast::Expr::Binary { .. }));
            }
            _ => panic!("expected SharedLet with binary expression"),
        }
    }

    #[test]
    fn test_atomic_block_multi_statement() {
        let src = "interface i {\n  f: func() {\n    atomic {\n      let x = 1;\n      x + 1;\n    };\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!(),
        };
        let func = match &iface.items[0] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!(),
        };
        let body = func.body.as_ref().unwrap();
        match &body.statements[0] {
            crate::ast::Statement::Expr(crate::ast::Expr::AtomicBlock { body, .. }) => {
                assert_eq!(body.len(), 2, "atomic block should have 2 statements");
            }
            other => panic!("expected AtomicBlock, got: {:?}", other),
        }
    }

    #[test]
    fn test_shared_let_and_atomic_block_together() {
        let src = "interface i {\n  f: func() {\n    shared let counter = 0;\n    atomic {\n      counter + 1;\n    };\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!(),
        };
        let func = match &iface.items[0] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!(),
        };
        let body = func.body.as_ref().unwrap();
        assert_eq!(body.statements.len(), 2);
        assert!(matches!(&body.statements[0], crate::ast::Statement::SharedLet { .. }));
        assert!(matches!(&body.statements[1], crate::ast::Statement::Expr(crate::ast::Expr::AtomicBlock { .. })));
    }

    // ================================================================
    // Phase 13f: Thread join parsing
    // ================================================================

    #[test]
    fn test_thread_join_parse() {
        let src = "interface i {\n  f: func() {\n    let tid = spawn { 1; };\n    thread.join(tid);\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!("expected Interface"),
        };
        let func = match &iface.items[0] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!("expected Func"),
        };
        let body = func.body.as_ref().expect("should have body");
        assert_eq!(body.statements.len(), 2);
        match &body.statements[1] {
            crate::ast::Statement::Expr(crate::ast::Expr::ThreadJoin { .. }) => {}
            other => panic!("expected ThreadJoin, got: {:?}", other),
        }
    }

    #[test]
    fn test_thread_join_standalone() {
        let src = "interface i {\n  f: func() {\n    let tid = spawn { 42; };\n    let result = thread.join(tid);\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!(),
        };
        let func = match &iface.items[0] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!(),
        };
        let body = func.body.as_ref().unwrap();
        match &body.statements[1] {
            crate::ast::Statement::Let { name, value } => {
                assert_eq!(name.name, "result");
                assert!(matches!(value, crate::ast::Expr::ThreadJoin { .. }));
            }
            _ => panic!("expected let binding with thread.join"),
        }
    }

    // ================================================================
    // Phase 13g: Compound assignments and atomic desugaring
    // ================================================================

    #[test]
    fn test_compound_assign_add_parse() {
        let src = "interface i {\n  f: func() {\n    let x = 0;\n    x += 1;\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!(),
        };
        let func = match &iface.items[0] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!(),
        };
        let body = func.body.as_ref().unwrap();
        match &body.statements[1] {
            crate::ast::Statement::CompoundAssign { name, op, .. } => {
                assert_eq!(name.name, "x");
                assert_eq!(*op, crate::ast::BinOp::Add);
            }
            other => panic!("expected CompoundAssign, got {:?}", other),
        }
    }

    #[test]
    fn test_compound_assign_sub_parse() {
        let src = "interface i {\n  f: func() {\n    let x = 10;\n    x -= 3;\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!(),
        };
        let func = match &iface.items[0] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!(),
        };
        let body = func.body.as_ref().unwrap();
        match &body.statements[1] {
            crate::ast::Statement::CompoundAssign { name, op, .. } => {
                assert_eq!(name.name, "x");
                assert_eq!(*op, crate::ast::BinOp::Sub);
            }
            other => panic!("expected CompoundAssign, got {:?}", other),
        }
    }

    #[test]
    fn test_atomic_block_with_compound_assign() {
        let src = "interface i {\n  f: func() {\n    shared let counter = 0;\n    atomic { counter += 1; };\n  }\n}";
        let (ast, errors) = parse(src);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let ast = ast.expect("should parse");
        let iface = match &ast.items[0] {
            crate::ast::TopLevelItem::Interface(i) => i,
            _ => panic!(),
        };
        let func = match &iface.items[0] {
            crate::ast::InterfaceItem::Func(f) => f,
            _ => panic!(),
        };
        let body = func.body.as_ref().unwrap();
        match &body.statements[1] {
            crate::ast::Statement::Expr(crate::ast::Expr::AtomicBlock { body, .. }) => {
                assert_eq!(body.len(), 1);
                assert!(matches!(&body[0], crate::ast::Statement::CompoundAssign { .. }));
            }
            other => panic!("expected AtomicBlock with CompoundAssign, got {:?}", other),
        }
    }
}
