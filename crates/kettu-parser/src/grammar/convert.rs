//! Conversion from grammar CST nodes to semantic AST nodes.
//!
//! The grammar module defines a concrete syntax tree (CST) that maps 1:1
//! to source text via rust-sitter annotations. This module converts those
//! CST nodes into the semantic AST defined in `crate::ast`, which downstream
//! crates consume.
//!
//! Span information is extracted from `Spanned<T>` wrappers that rust-sitter
//! populates with Tree-sitter byte ranges during parsing.

use crate::ast;
use crate::grammar;
use rust_sitter::Spanned;
use std::ops::Range;

// ============================================================================
// Span helpers
// ============================================================================

/// Extract byte span from a Spanned wrapper
fn span<T>(s: &Spanned<T>) -> Range<usize> {
    s.position.bytes.clone()
}

/// Convert a Spanned<Box<Expr>> to a semantic Expr, using its span
fn sexpr(s: &Spanned<Box<grammar::Expr>>) -> ast::Expr {
    expr_with_span(&s.value, span(s))
}

/// Convert a Spanned<Expr> (flat) to a semantic Expr, using its span
fn sexpr_flat(s: &Spanned<grammar::Expr>) -> ast::Expr {
    expr_with_span(&s.value, span(s))
}

/// Convert a Spanned<LambdaExpr> to a semantic lambda Expr, using its span
fn slambda_expr(s: &Spanned<grammar::LambdaExpr>) -> ast::Expr {
    ast::Expr::Lambda {
        params: s.value.params.iter().map(spanned_id).collect(),
        body: Box::new(sexpr(&s.value.body)),
        captures: vec![],
        span: span(s),
    }
}

fn lower_builtin_call(
    func: Box<ast::Expr>,
    mut args: Vec<ast::Expr>,
    span: Range<usize>,
) -> ast::Expr {
    if let ast::Expr::Ident(id) = func.as_ref() {
        match id.name.as_str() {
            "map" if args.len() == 2 => {
                let lambda = args.pop().expect("map lambda arg");
                let list = args.pop().expect("map list arg");
                return ast::Expr::Map {
                    list: Box::new(list),
                    lambda: Box::new(lambda),
                    span,
                };
            }
            "filter" if args.len() == 2 => {
                let lambda = args.pop().expect("filter lambda arg");
                let list = args.pop().expect("filter list arg");
                return ast::Expr::Filter {
                    list: Box::new(list),
                    lambda: Box::new(lambda),
                    span,
                };
            }
            "reduce" if args.len() == 3 => {
                let lambda = args.pop().expect("reduce lambda arg");
                let init = args.pop().expect("reduce init arg");
                let list = args.pop().expect("reduce list arg");
                return ast::Expr::Reduce {
                    list: Box::new(list),
                    init: Box::new(init),
                    lambda: Box::new(lambda),
                    span,
                };
            }
            _ => {}
        }
    }
    ast::Expr::Call { func, args, span }
}

fn lower_variant_constructor_call(
    func: ast::Expr,
    mut args: Vec<ast::Expr>,
    span: Range<usize>,
) -> ast::Expr {
    if let ast::Expr::VariantLiteral {
        type_name,
        case_name,
        payload: None,
        ..
    } = func
    {
        return match args.len() {
            0 => ast::Expr::VariantLiteral {
                type_name,
                case_name,
                payload: None,
                span,
            },
            1 => ast::Expr::VariantLiteral {
                type_name,
                case_name,
                payload: Some(Box::new(args.remove(0))),
                span,
            },
            _ => ast::Expr::Call {
                func: Box::new(ast::Expr::VariantLiteral {
                    type_name,
                    case_name,
                    payload: None,
                    span: span.clone(),
                }),
                args,
                span,
            },
        };
    }

    ast::Expr::Call {
        func: Box::new(func),
        args,
        span,
    }
}

/// Convert a Spanned<Box<TyNode>> to a semantic Ty, using its span
fn sty(s: &Spanned<Box<grammar::TyNode>>) -> ast::Ty {
    ty_node_with_span(&s.value, span(s))
}

/// Convert a Spanned<TyNode> (flat) to a semantic Ty, using its span
fn sty_flat(s: &Spanned<grammar::TyNode>) -> ast::Ty {
    ty_node_with_span(&s.value, span(s))
}

/// Convert a Spanned<String> identifier to an AST Id with real span
fn spanned_id(name: &Spanned<String>) -> ast::Id {
    let n = if name.value.starts_with('%') {
        name.value[1..].to_string()
    } else {
        name.value.clone()
    };
    ast::Id::new(n, span(name))
}

// ============================================================================
// Top-level conversion
// ============================================================================

/// Convert a grammar WitFile to a semantic AST WitFile
pub fn wit_file(cst: &grammar::WitFile) -> ast::WitFile {
    ast::WitFile {
        package: cst.package.as_ref().map(package_decl),
        items: cst.items.iter().map(top_level_item).collect(),
    }
}

fn package_decl(cst: &Spanned<grammar::PackageDecl>) -> ast::PackageDecl {
    let path = package_path(&cst.value.path);
    ast::PackageDecl {
        path,
        span: span(cst),
    }
}

fn package_path(cst: &grammar::PackagePath) -> ast::PackagePath {
    ast::PackagePath {
        namespace: vec![spanned_id(&cst.namespace)],
        name: vec![spanned_id(&cst.name)],
        version: cst.version.as_ref().map(|v| version(&v.version)),
    }
}

fn use_path_ref(cst: &grammar::UsePathRef) -> ast::UsePath {
    match cst {
        grammar::UsePathRef::PackageQualified(p) => ast::UsePath {
            package: Some(ast::PackagePath {
                namespace: vec![spanned_id(&p.package.namespace)],
                name: vec![spanned_id(&p.package.name)],
                version: p.version.as_ref().map(|v| version(&v.version)),
            }),
            interface: spanned_id(&p.interface),
        },
        grammar::UsePathRef::Local(l) => ast::UsePath {
            package: None,
            interface: spanned_id(&l.interface),
        },
    }
}

fn top_level_item(cst: &Spanned<grammar::TopLevelItem>) -> ast::TopLevelItem {
    match &cst.value {
        grammar::TopLevelItem::Use(u) => ast::TopLevelItem::Use(top_level_use(u)),
        grammar::TopLevelItem::Interface(i) => ast::TopLevelItem::Interface(interface_def(i)),
        grammar::TopLevelItem::World(w) => ast::TopLevelItem::World(world_def(w)),
    }
}

fn top_level_use(cst: &Spanned<grammar::TopLevelUse>) -> ast::TopLevelUse {
    ast::TopLevelUse {
        path: use_path_ref(&cst.value.path),
        alias: cst.value.alias.as_ref().map(|a| spanned_id(&a.alias)),
        span: span(cst),
    }
}

fn gate(cst: &grammar::Gate) -> ast::Gate {
    match cst {
        grammar::Gate::Since(g) => ast::Gate::Since {
            version: version(&g.version),
        },
        grammar::Gate::Unstable(g) => ast::Gate::Unstable {
            feature: spanned_id(&g.feature),
        },
        grammar::Gate::Deprecated(g) => ast::Gate::Deprecated {
            version: version(&g.version),
        },
        grammar::Gate::Test(_) => ast::Gate::Test,
    }
}

fn version(cst: &Spanned<grammar::Version>) -> ast::Version {
    let raw = cst.value.raw.value.as_str();
    let (core, prerelease) = match raw.split_once('-') {
        Some((core, suffix)) => (core, Some(suffix.to_string())),
        None => (raw, None),
    };

    let mut it = core.split('.');
    let major = it.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
    let minor = it.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
    let patch = it.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);

    ast::Version {
        major,
        minor,
        patch,
        prerelease,
        span: span(cst),
    }
}

// ============================================================================
// Interface and type definitions
// ============================================================================

fn interface_def(cst: &Spanned<grammar::InterfaceDef>) -> ast::Interface {
    ast::Interface {
        gates: cst.value.gates.iter().map(gate).collect(),
        name: spanned_id(&cst.value.name),
        items: cst.value.items.iter().map(interface_item).collect(),
        span: span(cst),
    }
}

fn interface_item(cst: &Spanned<grammar::InterfaceItem>) -> ast::InterfaceItem {
    match &cst.value {
        grammar::InterfaceItem::TypeDef(td) => ast::InterfaceItem::TypeDef(type_def(td)),
        grammar::InterfaceItem::Use(u) => ast::InterfaceItem::Use(use_statement(u)),
        grammar::InterfaceItem::Func(f) => ast::InterfaceItem::Func(func_def(f)),
    }
}

fn use_statement(cst: &Spanned<grammar::UseStatement>) -> ast::UseStatement {
    ast::UseStatement {
        path: use_path_ref(&cst.value.path),
        names: cst
            .value
            .items
            .iter()
            .map(|item| ast::UseItem {
                name: spanned_id(&item.name),
                alias: item.alias.as_ref().map(|a| spanned_id(&a.name)),
            })
            .collect(),
        span: span(cst),
    }
}

fn type_def(cst: &Spanned<grammar::TypeDef>) -> ast::TypeDef {
    let s = span(cst);
    match &cst.value {
        grammar::TypeDef::Record(r) => ast::TypeDef {
            gates: vec![],
            kind: ast::TypeDefKind::Record {
                name: spanned_id(&r.name),
                type_params: type_params_list(&r.type_params),
                fields: r.fields.iter().map(record_field).collect(),
            },
            span: s,
        },
        grammar::TypeDef::Variant(v) => ast::TypeDef {
            gates: vec![],
            kind: ast::TypeDefKind::Variant {
                name: spanned_id(&v.name),
                type_params: type_params_list(&v.type_params),
                cases: v.cases.iter().map(variant_case).collect(),
            },
            span: s,
        },
        grammar::TypeDef::Enum(e) => ast::TypeDef {
            gates: vec![],
            kind: ast::TypeDefKind::Enum {
                name: spanned_id(&e.name),
                cases: e.cases.iter().map(spanned_id).collect(),
            },
            span: s,
        },
        grammar::TypeDef::Flags(f) => ast::TypeDef {
            gates: vec![],
            kind: ast::TypeDefKind::Flags {
                name: spanned_id(&f.name),
                flags: f.flags.iter().map(spanned_id).collect(),
            },
            span: s,
        },
        grammar::TypeDef::TypeAlias(a) => ast::TypeDef {
            gates: vec![],
            kind: ast::TypeDefKind::Alias {
                name: spanned_id(&a.name),
                type_params: type_params_list(&a.type_params),
                ty: sty_flat(&a.ty),
            },
            span: s,
        },
        grammar::TypeDef::Resource(r) => {
            let (name, methods) = match r {
                grammar::ResourceDef::Simple(s) => (spanned_id(&s.name), vec![]),
                grammar::ResourceDef::WithMethods(m) => (
                    spanned_id(&m.name),
                    m.methods.iter().map(resource_method_spanned).collect(),
                ),
            };
            ast::TypeDef {
                gates: vec![],
                kind: ast::TypeDefKind::Resource { name, methods },
                span: s,
            }
        }
    }
}

fn type_params_list(tp: &Option<grammar::TypeParams>) -> Vec<ast::Id> {
    match tp {
        Some(tp) => tp.params.iter().map(spanned_id).collect(),
        None => vec![],
    }
}

fn record_field(cst: &grammar::RecordField) -> ast::RecordField {
    ast::RecordField {
        name: spanned_id(&cst.name),
        ty: sty_flat(&cst.ty),
    }
}

fn variant_case(cst: &grammar::VariantCase) -> ast::VariantCase {
    ast::VariantCase {
        name: spanned_id(&cst.name),
        ty: cst.payload.as_ref().map(|p| sty_flat(&p.ty)),
    }
}

fn resource_method_spanned(cst: &Spanned<grammar::ResourceMethod>) -> ast::ResourceMethod {
    let method_span = span(cst);
    match &cst.value {
        grammar::ResourceMethod::Constructor(c) => ast::ResourceMethod::Constructor {
            params: param_list(&c.params),
            result: None,
            body: c.body.as_ref().map(spanned_func_body),
            span: method_span,
        },
        grammar::ResourceMethod::Static(s) => ast::ResourceMethod::Static(ast::Func {
            gates: vec![],
            name: spanned_id(&s.name),
            type_params: vec![],
            is_async: false,
            params: param_list(&s.params),
            result: s.result.as_ref().map(|r| sty_flat(&r.ty)),
            body: s.body.as_ref().map(spanned_func_body),
            span: method_span,
        }),
        grammar::ResourceMethod::Instance(i) => ast::ResourceMethod::Method(ast::Func {
            gates: vec![],
            name: spanned_id(&i.name),
            type_params: vec![],
            is_async: false,
            params: param_list(&i.params),
            result: i.result.as_ref().map(|r| sty_flat(&r.ty)),
            body: i.body.as_ref().map(spanned_func_body),
            span: method_span,
        }),
    }
}

// ============================================================================
// Type nodes
// ============================================================================

/// Convert a grammar TyNode to a semantic Ty with a given outer span
fn ty_node_with_span(cst: &grammar::TyNode, outer_span: Range<usize>) -> ast::Ty {
    match cst {
        grammar::TyNode::U8 => ast::Ty::Primitive(ast::PrimitiveTy::U8, outer_span),
        grammar::TyNode::U16 => ast::Ty::Primitive(ast::PrimitiveTy::U16, outer_span),
        grammar::TyNode::U32 => ast::Ty::Primitive(ast::PrimitiveTy::U32, outer_span),
        grammar::TyNode::U64 => ast::Ty::Primitive(ast::PrimitiveTy::U64, outer_span),
        grammar::TyNode::S8 => ast::Ty::Primitive(ast::PrimitiveTy::S8, outer_span),
        grammar::TyNode::S16 => ast::Ty::Primitive(ast::PrimitiveTy::S16, outer_span),
        grammar::TyNode::S32 => ast::Ty::Primitive(ast::PrimitiveTy::S32, outer_span),
        grammar::TyNode::S64 => ast::Ty::Primitive(ast::PrimitiveTy::S64, outer_span),
        grammar::TyNode::F32 => ast::Ty::Primitive(ast::PrimitiveTy::F32, outer_span),
        grammar::TyNode::F64 => ast::Ty::Primitive(ast::PrimitiveTy::F64, outer_span),
        grammar::TyNode::Bool => ast::Ty::Primitive(ast::PrimitiveTy::Bool, outer_span),
        grammar::TyNode::Char => ast::Ty::Primitive(ast::PrimitiveTy::Char, outer_span),
        grammar::TyNode::String_ => ast::Ty::Primitive(ast::PrimitiveTy::String, outer_span),
        grammar::TyNode::List(l) => ast::Ty::List {
            element: Box::new(sty(&l.element)),
            size: None,
            span: outer_span,
        },
        grammar::TyNode::Option_(o) => ast::Ty::Option {
            inner: Box::new(sty(&o.inner)),
            span: outer_span,
        },
        grammar::TyNode::Result_(r) => {
            let (ok, err) = match &r.args {
                Some(args) => (
                    Some(Box::new(sty(&args.ok))),
                    args.err.as_ref().map(|e| Box::new(sty(&e.ty))),
                ),
                None => (None, None),
            };
            ast::Ty::Result {
                ok,
                err,
                span: outer_span,
            }
        }
        grammar::TyNode::Tuple(t) => ast::Ty::Tuple {
            elements: t.elements.iter().map(sty_flat).collect(),
            span: outer_span,
        },
        grammar::TyNode::Future(f) => ast::Ty::Future {
            inner: f.inner.as_ref().map(|a| Box::new(sty(&a.ty))),
            span: outer_span,
        },
        grammar::TyNode::Stream(s) => ast::Ty::Stream {
            inner: s.inner.as_ref().map(|a| Box::new(sty(&a.ty))),
            span: outer_span,
        },
        grammar::TyNode::Named(n) => match &n.args {
            Some(args) => ast::Ty::Generic {
                name: spanned_id(&n.name),
                args: args.args.iter().map(sty_flat).collect(),
                span: outer_span,
            },
            None => ast::Ty::Named(spanned_id(&n.name)),
        },
    }
}

// ============================================================================
// Functions, parameters, bodies
// ============================================================================

fn func_def(cst: &Spanned<grammar::FuncDef>) -> ast::Func {
    ast::Func {
        gates: cst.value.gates.iter().map(gate).collect(),
        name: spanned_id(&cst.value.name),
        type_params: type_params_list(&cst.value.type_params),
        is_async: cst.value.is_async.is_some(),
        params: param_list(&cst.value.params),
        result: cst.value.result.as_ref().map(|r| sty_flat(&r.ty)),
        body: cst.value.body.as_ref().map(spanned_func_body),
        span: span(cst),
    }
}

fn param_list(cst: &grammar::ParamList) -> Vec<ast::Param> {
    cst.params
        .iter()
        .map(|p| ast::Param {
            name: spanned_id(&p.name),
            ty: sty_flat(&p.ty),
        })
        .collect()
}

fn spanned_func_body(cst: &Spanned<grammar::FuncBody>) -> ast::FuncBody {
    ast::FuncBody {
        statements: cst.value.statements.iter().map(spanned_stmt).collect(),
        span: span(cst),
    }
}

fn spanned_stmt(cst: &Spanned<grammar::Stmt>) -> ast::Statement {
    stmt(&cst.value)
}

fn stmt(cst: &grammar::Stmt) -> ast::Statement {
    match cst {
        grammar::Stmt::Let(l) => ast::Statement::Let {
            name: spanned_id(&l.name),
            value: sexpr_flat(&l.value),
        },
        grammar::Stmt::Assign(a) => ast::Statement::Assign {
            name: spanned_id(&a.name),
            value: sexpr_flat(&a.value),
        },
        grammar::Stmt::AddAssign(a) => ast::Statement::CompoundAssign {
            name: spanned_id(&a.name),
            op: ast::BinOp::Add,
            value: sexpr_flat(&a.value),
        },
        grammar::Stmt::SubAssign(a) => ast::Statement::CompoundAssign {
            name: spanned_id(&a.name),
            op: ast::BinOp::Sub,
            value: sexpr_flat(&a.value),
        },
        grammar::Stmt::ReturnValue(r) => ast::Statement::Return(Some(sexpr_flat(&r.value))),
        grammar::Stmt::ReturnVoid(_) => ast::Statement::Return(None),
        grammar::Stmt::Break(_) => ast::Statement::Break { condition: None },
        grammar::Stmt::Continue(_) => ast::Statement::Continue { condition: None },
        grammar::Stmt::Expr(e) => ast::Statement::Expr(sexpr_flat(&e.expr)),
        grammar::Stmt::SharedLet(s) => ast::Statement::SharedLet {
            name: spanned_id(&s.name),
            initial_value: sexpr_flat(&s.value),
        },
    }
}

// ============================================================================
// Expressions
// ============================================================================

/// Convert a grammar Expr to a semantic Expr with a given outer span
fn expr_with_span(cst: &grammar::Expr, outer_span: Range<usize>) -> ast::Expr {
    match cst {
        grammar::Expr::Integer(val) => ast::Expr::Integer(*val, outer_span),
        grammar::Expr::String_(lit) => {
            // Strip surrounding quotes
            let inner = if lit.len() >= 2 {
                &lit[1..lit.len() - 1]
            } else {
                lit
            };
            ast::Expr::String(inner.to_string(), outer_span)
        }
        grammar::Expr::True => ast::Expr::Bool(true, outer_span),
        grammar::Expr::False => ast::Expr::Bool(false, outer_span),
        grammar::Expr::Ident(name) => ast::Expr::Ident(spanned_id(name)),
        grammar::Expr::Parens(p) => sexpr(&p.inner),
        grammar::Expr::Not(_, e) => ast::Expr::Not(Box::new(sexpr(e)), outer_span),
        grammar::Expr::Assert(_, e) => ast::Expr::Assert(Box::new(sexpr(e)), outer_span),
        grammar::Expr::Await(_, e) => ast::Expr::Await {
            expr: Box::new(sexpr(e)),
            span: outer_span,
        },

        // Binary operators
        grammar::Expr::Or(l, _, r) => ast::Expr::Binary {
            lhs: Box::new(sexpr(l)),
            op: ast::BinOp::Or,
            rhs: Box::new(sexpr(r)),
            span: outer_span,
        },
        grammar::Expr::And(l, _, r) => ast::Expr::Binary {
            lhs: Box::new(sexpr(l)),
            op: ast::BinOp::And,
            rhs: Box::new(sexpr(r)),
            span: outer_span,
        },
        grammar::Expr::Eq(l, _, r) => ast::Expr::Binary {
            lhs: Box::new(sexpr(l)),
            op: ast::BinOp::Eq,
            rhs: Box::new(sexpr(r)),
            span: outer_span,
        },
        grammar::Expr::Ne(l, _, r) => ast::Expr::Binary {
            lhs: Box::new(sexpr(l)),
            op: ast::BinOp::Ne,
            rhs: Box::new(sexpr(r)),
            span: outer_span,
        },
        grammar::Expr::Lt(l, _, r) => ast::Expr::Binary {
            lhs: Box::new(sexpr(l)),
            op: ast::BinOp::Lt,
            rhs: Box::new(sexpr(r)),
            span: outer_span,
        },
        grammar::Expr::Le(l, _, r) => ast::Expr::Binary {
            lhs: Box::new(sexpr(l)),
            op: ast::BinOp::Le,
            rhs: Box::new(sexpr(r)),
            span: outer_span,
        },
        grammar::Expr::Gt(l, _, r) => ast::Expr::Binary {
            lhs: Box::new(sexpr(l)),
            op: ast::BinOp::Gt,
            rhs: Box::new(sexpr(r)),
            span: outer_span,
        },
        grammar::Expr::Ge(l, _, r) => ast::Expr::Binary {
            lhs: Box::new(sexpr(l)),
            op: ast::BinOp::Ge,
            rhs: Box::new(sexpr(r)),
            span: outer_span,
        },
        grammar::Expr::Add(l, _, r) => ast::Expr::Binary {
            lhs: Box::new(sexpr(l)),
            op: ast::BinOp::Add,
            rhs: Box::new(sexpr(r)),
            span: outer_span,
        },
        grammar::Expr::Sub(l, _, r) => ast::Expr::Binary {
            lhs: Box::new(sexpr(l)),
            op: ast::BinOp::Sub,
            rhs: Box::new(sexpr(r)),
            span: outer_span,
        },
        grammar::Expr::Mul(l, _, r) => ast::Expr::Binary {
            lhs: Box::new(sexpr(l)),
            op: ast::BinOp::Mul,
            rhs: Box::new(sexpr(r)),
            span: outer_span,
        },
        grammar::Expr::Div(l, _, r) => ast::Expr::Binary {
            lhs: Box::new(sexpr(l)),
            op: ast::BinOp::Div,
            rhs: Box::new(sexpr(r)),
            span: outer_span,
        },

        // Postfix
        grammar::Expr::Call(func_expr, call_args) => {
            let func = sexpr(func_expr);
            let args: Vec<ast::Expr> = call_args.args.iter().map(sexpr_flat).collect();
            lower_variant_constructor_call(func, args, outer_span)
        }
        grammar::Expr::TrailingCall(func_expr, trailing) => {
            let lambda_arg = slambda_expr(&trailing.lambda);
            match sexpr(func_expr) {
                ast::Expr::Call { func, mut args, .. } => {
                    args.push(lambda_arg);
                    lower_builtin_call(func, args, outer_span)
                }
                other => lower_builtin_call(Box::new(other), vec![lambda_arg], outer_span),
            }
        }
        grammar::Expr::Try(e, _) => ast::Expr::Try {
            expr: Box::new(sexpr(e)),
            span: outer_span,
        },
        grammar::Expr::OptionalChain(e, _, field_name) => ast::Expr::OptionalChain {
            expr: Box::new(sexpr(e)),
            field: spanned_id(field_name),
            span: outer_span,
        },
        grammar::Expr::Field(e, _, field_name) => ast::Expr::Field {
            expr: Box::new(sexpr(e)),
            field: spanned_id(field_name),
            span: outer_span,
        },
        grammar::Expr::Index(e, _, idx, _) => ast::Expr::Index {
            expr: Box::new(sexpr(e)),
            index: Box::new(sexpr(idx)),
            span: outer_span,
        },
        grammar::Expr::Slice(e, _, start, _, end, _) => ast::Expr::Slice {
            expr: Box::new(sexpr(e)),
            start: Box::new(sexpr(start)),
            end: Box::new(sexpr(end)),
            span: outer_span,
        },

        // Compound
        grammar::Expr::If(i) => ast::Expr::If {
            cond: Box::new(sexpr(&i.condition)),
            then_branch: i.then_body.iter().map(stmt).collect(),
            else_branch: i
                .else_branch
                .as_ref()
                .map(|e| e.body.iter().map(stmt).collect()),
            span: outer_span,
        },
        grammar::Expr::While(w) => ast::Expr::While {
            condition: Box::new(sexpr(&w.condition)),
            body: w.body.iter().map(stmt).collect(),
            span: outer_span,
        },
        grammar::Expr::For(f) => {
            let descending = matches!(f.direction, grammar::ForDirection::Downto);
            ast::Expr::For {
                variable: spanned_id(&f.variable),
                range: Box::new(ast::Expr::Range {
                    start: Box::new(sexpr(&f.start)),
                    end: Box::new(sexpr(&f.end)),
                    step: f.step.as_ref().map(|s| Box::new(sexpr(&s.value))),
                    descending,
                    span: outer_span.clone(),
                }),
                body: f.body.iter().map(stmt).collect(),
                span: outer_span,
            }
        }
        grammar::Expr::ForEach(f) => ast::Expr::ForEach {
            variable: spanned_id(&f.variable),
            collection: Box::new(sexpr(&f.collection)),
            body: f.body.iter().map(stmt).collect(),
            span: outer_span,
        },
        grammar::Expr::SimdForEach(f) => ast::Expr::SimdForEach {
            variable: spanned_id(&f.variable),
            collection: Box::new(sexpr(&f.collection)),
            body: f.body.iter().map(stmt).collect(),
            span: outer_span,
        },
        grammar::Expr::Match(m) => ast::Expr::Match {
            scrutinee: Box::new(sexpr(&m.scrutinee)),
            arms: m.arms.iter().map(spanned_match_arm).collect(),
            span: outer_span,
        },

        // Built-ins
        grammar::Expr::StrLen(s) => ast::Expr::StrLen(Box::new(sexpr(&s.expr)), outer_span),
        grammar::Expr::StrEq(s) => {
            ast::Expr::StrEq(Box::new(sexpr(&s.a)), Box::new(sexpr(&s.b)), outer_span)
        }
        grammar::Expr::ListLen(l) => ast::Expr::ListLen(Box::new(sexpr(&l.expr)), outer_span),
        grammar::Expr::ListSet(l) => ast::Expr::ListSet(
            Box::new(sexpr(&l.arr)),
            Box::new(sexpr(&l.idx)),
            Box::new(sexpr(&l.val)),
            outer_span,
        ),
        grammar::Expr::ListPush(l) => {
            ast::Expr::ListPush(Box::new(sexpr(&l.arr)), Box::new(sexpr(&l.val)), outer_span)
        }

        // Lambda
        grammar::Expr::Lambda(l) => ast::Expr::Lambda {
            params: l.params.iter().map(spanned_id).collect(),
            body: Box::new(sexpr(&l.body)),
            captures: vec![],
            span: outer_span,
        },

        // HOFs
        grammar::Expr::Map(m) => {
            let list = sexpr(&m.list);
            match &m.lambda {
                Some(lambda) => ast::Expr::Map {
                    list: Box::new(list),
                    lambda: Box::new(sexpr(&lambda.lambda)),
                    span: outer_span,
                },
                None => ast::Expr::Call {
                    func: Box::new(ast::Expr::Ident(ast::Id::new("map", outer_span.clone()))),
                    args: vec![list],
                    span: outer_span,
                },
            }
        }
        grammar::Expr::Filter(f) => {
            let list = sexpr(&f.list);
            match &f.lambda {
                Some(lambda) => ast::Expr::Filter {
                    list: Box::new(list),
                    lambda: Box::new(sexpr(&lambda.lambda)),
                    span: outer_span,
                },
                None => ast::Expr::Call {
                    func: Box::new(ast::Expr::Ident(ast::Id::new("filter", outer_span.clone()))),
                    args: vec![list],
                    span: outer_span,
                },
            }
        }
        grammar::Expr::Reduce(r) => {
            let list = sexpr(&r.list);
            let init = sexpr(&r.init);
            match &r.lambda {
                Some(lambda) => ast::Expr::Reduce {
                    list: Box::new(list),
                    init: Box::new(init),
                    lambda: Box::new(sexpr(&lambda.lambda)),
                    span: outer_span,
                },
                None => ast::Expr::Call {
                    func: Box::new(ast::Expr::Ident(ast::Id::new("reduce", outer_span.clone()))),
                    args: vec![list, init],
                    span: outer_span,
                },
            }
        }

        // Literals
        grammar::Expr::ListLiteral(l) => ast::Expr::ListLiteral {
            elements: l.elements.iter().map(sexpr_flat).collect(),
            span: outer_span,
        },
        grammar::Expr::RecordLiteral(r) => ast::Expr::RecordLiteral {
            type_name: None,
            fields: r
                .fields
                .iter()
                .map(|f| (spanned_id(&f.name), Box::new(sexpr(&f.value))))
                .collect(),
            span: outer_span,
        },
        grammar::Expr::VariantLiteral(_, case_name) => ast::Expr::VariantLiteral {
            type_name: None,
            case_name: spanned_id(case_name),
            payload: None,
            span: outer_span,
        },
        grammar::Expr::QualifiedVariantLiteral(type_name, _, case_name) => {
            ast::Expr::VariantLiteral {
                type_name: Some(spanned_id(type_name)),
                case_name: spanned_id(case_name),
                payload: None,
                span: outer_span,
            }
        }

        // Option/Result constructors — use outer_span for the synthetic case names
        // since the keyword (some/none/ok/err) is part of the expression span
        grammar::Expr::Some_(s) => ast::Expr::VariantLiteral {
            type_name: None,
            case_name: ast::Id::new("some", outer_span.clone()),
            payload: Some(Box::new(sexpr(&s.value))),
            span: outer_span,
        },
        grammar::Expr::None_(_) => ast::Expr::VariantLiteral {
            type_name: None,
            case_name: ast::Id::new("none", outer_span.clone()),
            payload: None,
            span: outer_span,
        },
        grammar::Expr::Ok_(o) => ast::Expr::VariantLiteral {
            type_name: None,
            case_name: ast::Id::new("ok", outer_span.clone()),
            payload: Some(Box::new(sexpr(&o.value))),
            span: outer_span,
        },
        grammar::Expr::Err_(e) => ast::Expr::VariantLiteral {
            type_name: None,
            case_name: ast::Id::new("err", outer_span.clone()),
            payload: Some(Box::new(sexpr(&e.value))),
            span: outer_span,
        },

        // Atomic operations
        grammar::Expr::AtomicLoad(a) => ast::Expr::AtomicLoad {
            addr: Box::new(sexpr(&a.addr)),
            span: outer_span,
        },
        grammar::Expr::AtomicStore(a) => ast::Expr::AtomicStore {
            addr: Box::new(sexpr(&a.addr)),
            value: Box::new(sexpr(&a.value)),
            span: outer_span,
        },
        grammar::Expr::AtomicAdd(a) => ast::Expr::AtomicAdd {
            addr: Box::new(sexpr(&a.addr)),
            value: Box::new(sexpr(&a.value)),
            span: outer_span,
        },
        grammar::Expr::AtomicSub(a) => ast::Expr::AtomicSub {
            addr: Box::new(sexpr(&a.addr)),
            value: Box::new(sexpr(&a.value)),
            span: outer_span,
        },
        grammar::Expr::AtomicCmpxchg(a) => ast::Expr::AtomicCmpxchg {
            addr: Box::new(sexpr(&a.addr)),
            expected: Box::new(sexpr(&a.expected)),
            replacement: Box::new(sexpr(&a.replacement)),
            span: outer_span,
        },
        grammar::Expr::AtomicWait(a) => ast::Expr::AtomicWait {
            addr: Box::new(sexpr(&a.addr)),
            expected: Box::new(sexpr(&a.expected)),
            timeout: Box::new(sexpr(&a.timeout)),
            span: outer_span,
        },
        grammar::Expr::AtomicNotify(a) => ast::Expr::AtomicNotify {
            addr: Box::new(sexpr(&a.addr)),
            count: Box::new(sexpr(&a.count)),
            span: outer_span,
        },

        // Thread spawning
        grammar::Expr::Spawn(s) => ast::Expr::Spawn {
            body: s.body.iter().map(spanned_stmt).collect(),
            span: outer_span,
        },

        // Thread join
        grammar::Expr::ThreadJoin(t) => ast::Expr::ThreadJoin {
            tid: Box::new(sexpr(&t.tid)),
            span: outer_span,
        },

        // Atomic block sugar
        grammar::Expr::AtomicBlock(a) => ast::Expr::AtomicBlock {
            body: a.body.iter().map(spanned_stmt).collect(),
            span: outer_span,
        },

        // SIMD operations
        grammar::Expr::SimdV128(s) => convert_simd_expr(ast::SimdLane::V128, s.op.value.as_str(), &s.call_args.args, outer_span),
        grammar::Expr::SimdI8x16(s) => convert_simd_expr(ast::SimdLane::I8x16, s.op.value.as_str(), &s.call_args.args, outer_span),
        grammar::Expr::SimdI16x8(s) => convert_simd_expr(ast::SimdLane::I16x8, s.op.value.as_str(), &s.call_args.args, outer_span),
        grammar::Expr::SimdI32x4(s) => convert_simd_expr(ast::SimdLane::I32x4, s.op.value.as_str(), &s.call_args.args, outer_span),
        grammar::Expr::SimdI64x2(s) => convert_simd_expr(ast::SimdLane::I64x2, s.op.value.as_str(), &s.call_args.args, outer_span),
        grammar::Expr::SimdF32x4(s) => convert_simd_expr(ast::SimdLane::F32x4, s.op.value.as_str(), &s.call_args.args, outer_span),
        grammar::Expr::SimdF64x2(s) => convert_simd_expr(ast::SimdLane::F64x2, s.op.value.as_str(), &s.call_args.args, outer_span),
    }
}

/// Convert a SIMD grammar expression to `Expr::SimdOp`
fn convert_simd_expr(
    lane: ast::SimdLane,
    op_name: &str,
    args: &[Spanned<grammar::Expr>],
    span: Range<usize>,
) -> ast::Expr {
    let op = parse_simd_op(op_name);
    let mut converted_args: Vec<ast::Expr> = Vec::new();
    let mut lane_idx: Option<u8> = None;

    for (i, arg) in args.iter().enumerate() {
        // For extract_lane/replace_lane, the lane index is a numeric arg
        if (op == ast::SimdOp::ExtractLane && i == args.len() - 1)
            || (op == ast::SimdOp::ReplaceLane && i == 1)
        {
            if let grammar::Expr::Integer(n) = &arg.value {
                lane_idx = Some(*n as u8);
                continue;
            }
        }
        converted_args.push(sexpr_flat(arg));
    }

    ast::Expr::SimdOp { lane, op, args: converted_args, lane_idx, span }
}

fn parse_simd_op(name: &str) -> ast::SimdOp {
    match name {
        "splat" => ast::SimdOp::Splat,
        "add" => ast::SimdOp::Add,
        "sub" => ast::SimdOp::Sub,
        "mul" => ast::SimdOp::Mul,
        "neg" => ast::SimdOp::Neg,
        "abs" => ast::SimdOp::Abs,
        "div" => ast::SimdOp::Div,
        "sqrt" => ast::SimdOp::Sqrt,
        "ceil" => ast::SimdOp::Ceil,
        "floor" => ast::SimdOp::Floor,
        "trunc" => ast::SimdOp::Trunc,
        "nearest" => ast::SimdOp::Nearest,
        "shl" => ast::SimdOp::Shl,
        "shr-s" | "shr_s" => ast::SimdOp::ShrS,
        "shr-u" | "shr_u" => ast::SimdOp::ShrU,
        "min" => ast::SimdOp::Min,
        "max" => ast::SimdOp::Max,
        "extract-lane" | "extract_lane" => ast::SimdOp::ExtractLane,
        "replace-lane" | "replace_lane" => ast::SimdOp::ReplaceLane,
        "eq" => ast::SimdOp::Eq,
        "ne" => ast::SimdOp::Ne,
        "lt-s" | "lt_s" => ast::SimdOp::LtS,
        "lt-u" | "lt_u" => ast::SimdOp::LtU,
        "gt-s" | "gt_s" => ast::SimdOp::GtS,
        "gt-u" | "gt_u" => ast::SimdOp::GtU,
        "le-s" | "le_s" => ast::SimdOp::LeS,
        "le-u" | "le_u" => ast::SimdOp::LeU,
        "ge-s" | "ge_s" => ast::SimdOp::GeS,
        "ge-u" | "ge_u" => ast::SimdOp::GeU,
        "lt" => ast::SimdOp::Lt,
        "gt" => ast::SimdOp::Gt,
        "le" => ast::SimdOp::Le,
        "ge" => ast::SimdOp::Ge,
        "and" => ast::SimdOp::And,
        "or" => ast::SimdOp::Or,
        "xor" => ast::SimdOp::Xor,
        "not" => ast::SimdOp::Not,
        "andnot" => ast::SimdOp::AndNot,
        "bitselect" => ast::SimdOp::Bitselect,
        "any-true" | "any_true" => ast::SimdOp::AnyTrue,
        "all-true" | "all_true" => ast::SimdOp::AllTrue,
        "bitmask" => ast::SimdOp::Bitmask,
        "swizzle" => ast::SimdOp::Swizzle,
        "load" => ast::SimdOp::Load,
        "store" => ast::SimdOp::Store,
        "popcnt" => ast::SimdOp::Popcnt,
        "avgr-u" | "avgr_u" => ast::SimdOp::AvgRU,
        "ext-mul-low-s" | "ext_mul_low_s" => ast::SimdOp::ExtMulLowS,
        "ext-mul-low-u" | "ext_mul_low_u" => ast::SimdOp::ExtMulLowU,
        "ext-mul-high-s" | "ext_mul_high_s" => ast::SimdOp::ExtMulHighS,
        "ext-mul-high-u" | "ext_mul_high_u" => ast::SimdOp::ExtMulHighU,
        "ext-add-pairwise-s" | "ext_add_pairwise_s" => ast::SimdOp::ExtAddPairwiseS,
        "ext-add-pairwise-u" | "ext_add_pairwise_u" => ast::SimdOp::ExtAddPairwiseU,
        "narrow-s" | "narrow_s" => ast::SimdOp::NarrowS,
        "narrow-u" | "narrow_u" => ast::SimdOp::NarrowU,
        "extend-low-s" | "extend_low_s" => ast::SimdOp::ExtendLowS,
        "extend-low-u" | "extend_low_u" => ast::SimdOp::ExtendLowU,
        "extend-high-s" | "extend_high_s" => ast::SimdOp::ExtendHighS,
        "extend-high-u" | "extend_high_u" => ast::SimdOp::ExtendHighU,
        "dot" => ast::SimdOp::Dot,
        _ => ast::SimdOp::Add, // fallback — checker will catch invalid ops
    }
}

// ============================================================================
// Patterns
// ============================================================================

fn spanned_match_arm(cst: &Spanned<grammar::MatchArm>) -> ast::MatchArm {
    let arm_span = span(cst);
    let pat_span = span(&cst.value.pattern);
    ast::MatchArm {
        pattern: pattern_with_span(&cst.value.pattern.value, pat_span),
        body: vec![ast::Statement::Expr(sexpr_flat(&cst.value.body))],
        span: arm_span,
    }
}

fn pattern_with_span(cst: &grammar::Pattern, outer_span: Range<usize>) -> ast::Pattern {
    match cst {
        grammar::Pattern::VariantPlain(v) => ast::Pattern::Variant {
            type_name: None,
            case_name: spanned_id(&v.case_name),
            binding: None,
            span: outer_span,
        },
        grammar::Pattern::VariantBound(v) => ast::Pattern::Variant {
            type_name: None,
            case_name: spanned_id(&v.case_name),
            binding: Some(spanned_id(&v.binding)),
            span: outer_span,
        },
        grammar::Pattern::QualifiedVariantPlain(v) => ast::Pattern::Variant {
            type_name: Some(spanned_id(&v.type_name)),
            case_name: spanned_id(&v.case_name),
            binding: None,
            span: outer_span,
        },
        grammar::Pattern::QualifiedVariantBound(v) => ast::Pattern::Variant {
            type_name: Some(spanned_id(&v.type_name)),
            case_name: spanned_id(&v.case_name),
            binding: Some(spanned_id(&v.binding)),
            span: outer_span,
        },
        grammar::Pattern::Wildcard => ast::Pattern::Wildcard(outer_span),
    }
}

// ============================================================================
// World
// ============================================================================

fn world_def(cst: &Spanned<grammar::WorldDef>) -> ast::World {
    ast::World {
        gates: cst.value.gates.iter().map(gate).collect(),
        name: spanned_id(&cst.value.name),
        items: cst.value.items.iter().map(world_item_decl).collect(),
        span: span(cst),
    }
}

fn world_item_decl(cst: &Spanned<grammar::WorldItemDecl>) -> ast::WorldItem {
    world_item(&cst.value.item, span(cst))
}

fn world_item(cst: &grammar::WorldItem, s: Range<usize>) -> ast::WorldItem {
    match cst {
        grammar::WorldItem::Import(i) => match i {
            grammar::WorldImport::Func(f) => {
                let func = ast::Func {
                    gates: vec![],
                    name: spanned_id(&f.name),
                    type_params: vec![],
                    is_async: false,
                    params: param_list(&f.params),
                    result: f.result.as_ref().map(|r| sty_flat(&r.ty)),
                    body: None,
                    span: s.clone(),
                };
                ast::WorldItem::Import(ast::ImportExport {
                    name: Some(spanned_id(&f.name)),
                    kind: ast::ImportExportKind::Func(func),
                    span: s,
                })
            }
            grammar::WorldImport::Path(p) => ast::WorldItem::Import(ast::ImportExport {
                name: None,
                kind: ast::ImportExportKind::Path(use_path_ref(&p.path)),
                span: s,
            }),
        },
        grammar::WorldItem::Export(e) => match e {
            grammar::WorldExport::Func(f) => {
                let func = ast::Func {
                    gates: vec![],
                    name: spanned_id(&f.name),
                    type_params: vec![],
                    is_async: false,
                    params: param_list(&f.params),
                    result: f.result.as_ref().map(|r| sty_flat(&r.ty)),
                    body: None,
                    span: s.clone(),
                };
                ast::WorldItem::Export(ast::ImportExport {
                    name: Some(spanned_id(&f.name)),
                    kind: ast::ImportExportKind::Func(func),
                    span: s,
                })
            }
            grammar::WorldExport::Path(p) => ast::WorldItem::Export(ast::ImportExport {
                name: None,
                kind: ast::ImportExportKind::Path(use_path_ref(&p.path)),
                span: s,
            }),
        },
        grammar::WorldItem::Include(i) => ast::WorldItem::Include(ast::IncludeStatement {
            path: use_path_ref(&i.path),
            with: vec![],
            span: s,
        }),
        grammar::WorldItem::Use(u) => ast::WorldItem::Use(ast::UseStatement {
            path: use_path_ref(&u.path),
            names: u
                .items
                .iter()
                .map(|item| ast::UseItem {
                    name: spanned_id(&item.name),
                    alias: item.alias.as_ref().map(|a| spanned_id(&a.name)),
                })
                .collect(),
            span: s,
        }),
    }
}
