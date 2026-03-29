//! Capture analysis for closures
//!
//! Identifies free variables in lambda expressions that need to be captured
//! from the enclosing scope.

use crate::ast::{Expr, Id, Pattern, Statement, StringPart};
use std::collections::HashSet;

/// Analyze an expression and populate captures for all Lambda nodes.
/// `scope` contains variables that are in scope (bound) at this point.
pub fn analyze_captures(expr: &mut Expr, scope: &HashSet<String>) {
    match expr {
        Expr::Lambda {
            params,
            body,
            captures,
            ..
        } => {
            // Build scope for lambda body: outer scope + params
            let mut inner_scope: HashSet<String> = scope.clone();
            for param in params.iter() {
                inner_scope.insert(param.name.clone());
            }

            // Find free variables in body
            let free_vars = find_free_variables(body, &inner_scope);

            // Populate captures with variables that are in outer scope
            *captures = free_vars
                .iter()
                .filter(|name| scope.contains(*name))
                .map(|name| Id {
                    name: name.clone(),
                    span: 0..0, // Synthetic span
                })
                .collect();

            // Recursively analyze nested lambdas
            analyze_captures(body, &inner_scope);
        }
        Expr::Binary { lhs, rhs, .. } => {
            analyze_captures(lhs, scope);
            analyze_captures(rhs, scope);
        }
        Expr::Call { func, args, .. } => {
            analyze_captures(func, scope);
            for arg in args {
                analyze_captures(arg, scope);
            }
        }
        Expr::Field { expr, .. } => {
            analyze_captures(expr, scope);
        }
        Expr::OptionalChain { expr, .. } => {
            analyze_captures(expr, scope);
        }
        Expr::Try { expr, .. } => {
            analyze_captures(expr, scope);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            analyze_captures(cond, scope);
            for stmt in then_branch {
                analyze_statement(stmt, scope);
            }
            if let Some(else_stmts) = else_branch {
                for stmt in else_stmts {
                    analyze_statement(stmt, scope);
                }
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            analyze_captures(scrutinee, scope);
            let mut arm_scope = scope.clone();
            for arm in arms {
                // Patterns may bind new variables
                let binding = get_pattern_binding(&arm.pattern);

                let was_present = if let Some(ref name) = binding {
                    !arm_scope.insert(name.clone())
                } else {
                    false
                };

                for stmt in &mut arm.body {
                    analyze_statement(stmt, &arm_scope);
                }

                // Cleanup scope for next arm
                if let Some(ref name) = binding {
                    if !was_present {
                        arm_scope.remove(name);
                    }
                }
            }
        }
        Expr::While {
            condition, body, ..
        } => {
            analyze_captures(condition, scope);
            for stmt in body {
                analyze_statement(stmt, scope);
            }
        }
        Expr::For {
            variable,
            range,
            body,
            ..
        } => {
            analyze_captures(range, scope);
            let mut inner_scope = scope.clone();
            inner_scope.insert(variable.name.clone());
            for stmt in body {
                analyze_statement(stmt, &inner_scope);
            }
        }
        Expr::ForEach {
            variable,
            collection,
            body,
            ..
        } => {
            analyze_captures(collection, scope);
            let mut inner_scope = scope.clone();
            inner_scope.insert(variable.name.clone());
            for stmt in body {
                analyze_statement(stmt, &inner_scope);
            }
        }
        Expr::Range {
            start, end, step, ..
        } => {
            analyze_captures(start, scope);
            analyze_captures(end, scope);
            if let Some(s) = step {
                analyze_captures(s, scope);
            }
        }
        Expr::Index { expr, index, .. } => {
            analyze_captures(expr, scope);
            analyze_captures(index, scope);
        }
        Expr::Slice {
            expr, start, end, ..
        } => {
            analyze_captures(expr, scope);
            analyze_captures(start, scope);
            analyze_captures(end, scope);
        }
        Expr::ListLiteral { elements, .. } => {
            for elem in elements {
                analyze_captures(elem, scope);
            }
        }
        Expr::RecordLiteral { fields, .. } => {
            for (_, value) in fields {
                analyze_captures(value, scope);
            }
        }
        Expr::Map { list, lambda, .. } => {
            analyze_captures(list, scope);
            analyze_captures(lambda, scope);
        }
        Expr::Filter { list, lambda, .. } => {
            analyze_captures(list, scope);
            analyze_captures(lambda, scope);
        }
        Expr::Reduce {
            list, init, lambda, ..
        } => {
            analyze_captures(list, scope);
            analyze_captures(init, scope);
            analyze_captures(lambda, scope);
        }
        Expr::InterpolatedString(parts, _) => {
            for part in parts {
                if let StringPart::Expr(e) = part {
                    analyze_captures(e, scope);
                }
            }
        }
        Expr::Assert(e, _) | Expr::Not(e, _) | Expr::StrLen(e, _) | Expr::ListLen(e, _) => {
            analyze_captures(e, scope);
        }
        Expr::StrEq(a, b, _) => {
            analyze_captures(a, scope);
            analyze_captures(b, scope);
        }
        Expr::ListSet(arr, idx, val, _) => {
            analyze_captures(arr, scope);
            analyze_captures(idx, scope);
            analyze_captures(val, scope);
        }
        Expr::ListPush(arr, val, _) => {
            analyze_captures(arr, scope);
            analyze_captures(val, scope);
        }
        Expr::VariantLiteral { payload, .. } => {
            if let Some(p) = payload {
                analyze_captures(p, scope);
            }
        }
        Expr::Await { expr, .. } => {
            analyze_captures(expr, scope);
        }
        // Terminals - no recursion needed
        Expr::Integer(_, _) | Expr::Bool(_, _) | Expr::String(_, _) | Expr::Ident(_) => {}
    }
}

fn analyze_statement(stmt: &mut Statement, scope: &HashSet<String>) {
    match stmt {
        Statement::Expr(e) => analyze_captures(e, scope),
        Statement::Let { value, .. } => analyze_captures(value, scope),
        Statement::Assign { value, .. } => analyze_captures(value, scope),
        Statement::Return(Some(e)) => analyze_captures(e, scope),
        Statement::Return(None) => {}
        Statement::Break { condition: Some(e) } => analyze_captures(e, scope),
        Statement::Break { condition: None } => {}
        Statement::Continue { condition: Some(e) } => analyze_captures(e, scope),
        Statement::Continue { condition: None } => {}
    }
}

/// Find all free (unbound) variable references in an expression.
pub fn find_free_variables(expr: &Expr, bound: &HashSet<String>) -> HashSet<String> {
    let mut free = HashSet::new();
    collect_free_variables(expr, bound, &mut free);
    free
}

fn collect_free_variables(expr: &Expr, bound: &HashSet<String>, free: &mut HashSet<String>) {
    match expr {
        Expr::Ident(id) => {
            if !bound.contains(&id.name) {
                free.insert(id.name.clone());
            }
        }
        Expr::Lambda { params, body, .. } => {
            // Lambda introduces new bindings
            let mut inner_bound = bound.clone();
            for param in params {
                inner_bound.insert(param.name.clone());
            }
            collect_free_variables(body, &inner_bound, free);
        }
        Expr::Binary { lhs, rhs, .. } => {
            collect_free_variables(lhs, bound, free);
            collect_free_variables(rhs, bound, free);
        }
        Expr::Call { func, args, .. } => {
            collect_free_variables(func, bound, free);
            for arg in args {
                collect_free_variables(arg, bound, free);
            }
        }
        Expr::Field { expr, .. } => {
            collect_free_variables(expr, bound, free);
        }
        Expr::OptionalChain { expr, .. } => {
            collect_free_variables(expr, bound, free);
        }
        Expr::Try { expr, .. } => {
            collect_free_variables(expr, bound, free);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_free_variables(cond, bound, free);
            for stmt in then_branch {
                collect_free_in_statement(stmt, bound, free);
            }
            if let Some(else_stmts) = else_branch {
                for stmt in else_stmts {
                    collect_free_in_statement(stmt, bound, free);
                }
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_free_variables(scrutinee, bound, free);
            let mut arm_bound = bound.clone();
            for arm in arms {
                let binding = get_pattern_binding(&arm.pattern);

                let was_present = if let Some(ref name) = binding {
                    !arm_bound.insert(name.clone())
                } else {
                    false
                };

                for stmt in &arm.body {
                    collect_free_in_statement(stmt, &arm_bound, free);
                }

                if let Some(ref name) = binding {
                    if !was_present {
                        arm_bound.remove(name);
                    }
                }
            }
        }
        Expr::While {
            condition, body, ..
        } => {
            collect_free_variables(condition, bound, free);
            for stmt in body {
                collect_free_in_statement(stmt, bound, free);
            }
        }
        Expr::For {
            variable,
            range,
            body,
            ..
        } => {
            collect_free_variables(range, bound, free);
            let mut inner_bound = bound.clone();
            inner_bound.insert(variable.name.clone());
            for stmt in body {
                collect_free_in_statement(stmt, &inner_bound, free);
            }
        }
        Expr::ForEach {
            variable,
            collection,
            body,
            ..
        } => {
            collect_free_variables(collection, bound, free);
            let mut inner_bound = bound.clone();
            inner_bound.insert(variable.name.clone());
            for stmt in body {
                collect_free_in_statement(stmt, &inner_bound, free);
            }
        }
        Expr::Range {
            start, end, step, ..
        } => {
            collect_free_variables(start, bound, free);
            collect_free_variables(end, bound, free);
            if let Some(s) = step {
                collect_free_variables(s, bound, free);
            }
        }
        Expr::Index { expr, index, .. } => {
            collect_free_variables(expr, bound, free);
            collect_free_variables(index, bound, free);
        }
        Expr::Slice {
            expr, start, end, ..
        } => {
            collect_free_variables(expr, bound, free);
            collect_free_variables(start, bound, free);
            collect_free_variables(end, bound, free);
        }
        Expr::ListLiteral { elements, .. } => {
            for elem in elements {
                collect_free_variables(elem, bound, free);
            }
        }
        Expr::RecordLiteral { fields, .. } => {
            for (_, value) in fields {
                collect_free_variables(value, bound, free);
            }
        }
        Expr::Map { list, lambda, .. } => {
            collect_free_variables(list, bound, free);
            collect_free_variables(lambda, bound, free);
        }
        Expr::Filter { list, lambda, .. } => {
            collect_free_variables(list, bound, free);
            collect_free_variables(lambda, bound, free);
        }
        Expr::Reduce {
            list, init, lambda, ..
        } => {
            collect_free_variables(list, bound, free);
            collect_free_variables(init, bound, free);
            collect_free_variables(lambda, bound, free);
        }
        Expr::InterpolatedString(parts, _) => {
            for part in parts {
                if let StringPart::Expr(e) = part {
                    collect_free_variables(e, bound, free);
                }
            }
        }
        Expr::Assert(e, _) | Expr::Not(e, _) | Expr::StrLen(e, _) | Expr::ListLen(e, _) => {
            collect_free_variables(e, bound, free);
        }
        Expr::StrEq(a, b, _) => {
            collect_free_variables(a, bound, free);
            collect_free_variables(b, bound, free);
        }
        Expr::ListSet(arr, idx, val, _) => {
            collect_free_variables(arr, bound, free);
            collect_free_variables(idx, bound, free);
            collect_free_variables(val, bound, free);
        }
        Expr::ListPush(arr, val, _) => {
            collect_free_variables(arr, bound, free);
            collect_free_variables(val, bound, free);
        }
        Expr::VariantLiteral { payload, .. } => {
            if let Some(p) = payload {
                collect_free_variables(p, bound, free);
            }
        }
        Expr::Await { expr, .. } => {
            collect_free_variables(expr, bound, free);
        }
        // Terminals
        Expr::Integer(_, _) | Expr::Bool(_, _) | Expr::String(_, _) => {}
    }
}

fn collect_free_in_statement(
    stmt: &Statement,
    bound: &HashSet<String>,
    free: &mut HashSet<String>,
) {
    match stmt {
        Statement::Expr(e) => collect_free_variables(e, bound, free),
        Statement::Let { value, .. } => collect_free_variables(value, bound, free),
        Statement::Assign { value, .. } => collect_free_variables(value, bound, free),
        Statement::Return(Some(e)) => collect_free_variables(e, bound, free),
        Statement::Return(None) => {}
        Statement::Break { condition: Some(e) } => collect_free_variables(e, bound, free),
        Statement::Break { condition: None } => {}
        Statement::Continue { condition: Some(e) } => collect_free_variables(e, bound, free),
        Statement::Continue { condition: None } => {}
    }
}

fn get_pattern_binding(pattern: &Pattern) -> Option<String> {
    match pattern {
        Pattern::Variant { binding, .. } => binding.as_ref().map(|id| id.name.clone()),
        Pattern::Wildcard(_) => None,
        Pattern::Literal(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_free_variables_simple() {
        // Create a simple Ident expression
        let id = Id {
            name: "x".to_string(),
            span: 0..1,
        };
        let expr = Expr::Ident(id);

        // x should be free when not in bound set
        let bound: HashSet<String> = HashSet::new();
        let free = find_free_variables(&expr, &bound);
        assert!(free.contains("x"));

        // x should not be free when in bound set
        let mut bound_with_x: HashSet<String> = HashSet::new();
        bound_with_x.insert("x".to_string());
        let free = find_free_variables(&expr, &bound_with_x);
        assert!(!free.contains("x"));
    }
}
