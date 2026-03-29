use kettu_parser::capture::{analyze_captures, find_free_variables};
use kettu_parser::ast::{Expr, Id, Pattern, Statement, MatchArm};
use std::collections::HashSet;
use std::time::Instant;

fn build_deep_match_expr(depth: usize) -> Expr {
    let mut stmts = vec![];
    for i in 0..10 {
        stmts.push(Statement::Let {
            variable: Id { name: format!("var{}", i), span: 0..0 },
            value: Expr::Ident(Id { name: "x".to_string(), span: 0..0 }),
            span: 0..0,
        });
    }
    let mut arms = vec![];
    for i in 0..10 {
        arms.push(MatchArm {
            pattern: Pattern::Wildcard(0..0),
            body: stmts.clone(),
            span: 0..0,
        });
    }

    let mut expr = Expr::Match {
        scrutinee: Box::new(Expr::Ident(Id { name: "x".to_string(), span: 0..0 })),
        arms: arms,
        span: 0..0,
    };

    for _ in 0..depth {
        expr = Expr::Match {
            scrutinee: Box::new(Expr::Ident(Id { name: "x".to_string(), span: 0..0 })),
            arms: vec![MatchArm {
                pattern: Pattern::Wildcard(0..0),
                body: vec![Statement::Expr(expr)],
                span: 0..0,
            }],
            span: 0..0,
        };
    }

    expr
}

fn main() {
    let mut expr = build_deep_match_expr(5000);
    let mut scope = HashSet::new();
    scope.insert("x".to_string());

    let start = Instant::now();
    analyze_captures(&mut expr, &scope);
    let duration = start.elapsed();
    println!("analyze_captures took: {:?}", duration);

    let bound = HashSet::new();
    let start = Instant::now();
    let _ = find_free_variables(&expr, &bound);
    let duration = start.elapsed();
    println!("find_free_variables took: {:?}", duration);
}
