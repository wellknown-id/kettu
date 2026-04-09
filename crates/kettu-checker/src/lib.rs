//! Kettu Type Checker
//!
//! Implements name resolution and type validation for Kettu/WIT.
//!
//! Key features:
//! - Two-pass resolution: collect all definitions first, then validate references
//! - Forward references allowed within the same file
//! - Recursive type detection (disallowed in WIT)
//! - Scoped type resolution (interface-local vs global)

use kettu_parser::ast::*;
use std::collections::{HashMap, HashSet};

// ============================================================================
// Public API
// ============================================================================

/// Check a WIT file for errors
pub fn check(file: &WitFile) -> Vec<Diagnostic> {
    let mut checker = Checker::new();
    checker.check_file(file);
    checker.diagnostics
}

/// Check a WIT file for errors with source content (for hush comments/constraints)
pub fn check_with_source(file: &WitFile, source: &str) -> Vec<Diagnostic> {
    let mut checker = Checker::new();
    checker.hush_comments = extract_hush_comments(source);
    checker.source_line_offsets = compute_line_offsets(source);
    checker.check_file(file);
    checker.diagnostics
}

fn compute_line_offsets(source: &str) -> Vec<usize> {
    let mut offsets = Vec::new();
    let mut pos = 0;
    for line in source.lines() {
        offsets.push(pos);
        pos += line.len() + 1;
    }
    offsets
}

/// Extract hush comments (constraint annotations) from source code
fn extract_hush_comments(source: &str) -> Vec<HushComment> {
    let mut comments = Vec::new();
    for (line_num, line) in source.lines().enumerate() {
        if let Some(caret_pos) = line.find("///") {
            let after_comment = &line[caret_pos + 3..];
            if let Some(ws_end) = after_comment.find(|c: char| !c.is_whitespace()) {
                let after_ws = &after_comment[ws_end..];
                if after_ws.starts_with('^') {
                    let text = &after_ws[1..].trim();
                    let caret_col = caret_pos + 3 + ws_end;
                    comments.push(HushComment {
                        line: line_num,
                        col: caret_col,
                        constraint: Expr::String(text.to_string(), Span::default()),
                        span: Span::default(),
                    });
                }
            }
        }
    }
    comments
}

/// Check multiple WIT files as a package
pub fn check_package(files: &[WitFile]) -> Vec<Diagnostic> {
    let mut checker = Checker::new();

    // First pass: collect all definitions from all files
    for file in files {
        checker.collect_definitions(file);
    }

    // Second pass: validate all references
    for file in files {
        checker.validate_file(file);
    }

    checker.diagnostics
}

// ============================================================================
// Diagnostic Types
// ============================================================================

/// Diagnostic message
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub message: String,
    pub span: Span,
    pub severity: Severity,
    pub code: DiagnosticCode,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>, span: Span, code: DiagnosticCode) -> Self {
        Self {
            message: message.into(),
            span,
            severity: Severity::Error,
            code,
        }
    }

    pub fn warning(message: impl Into<String>, span: Span, code: DiagnosticCode) -> Self {
        Self {
            message: message.into(),
            span,
            severity: Severity::Warning,
            code,
        }
    }

    pub fn info(message: impl Into<String>, span: Span, code: DiagnosticCode) -> Self {
        Self {
            message: message.into(),
            span,
            severity: Severity::Info,
            code,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticCode {
    UnknownType,
    UnknownInterface,
    UnknownResource,
    DuplicateDefinition,
    RecursiveType,
    InvalidUse,
    InvalidImport,
    InvalidExport,
    NestedPackageUnsupported,
    // Expression type errors
    TypeMismatch,
    UnknownVariable,
    InvalidOperator,
    DeprecatedFeature,
    UnstableFeature,
    // Constraint errors
    ConstraintViolation,
    ConstraintPropagation,
}

// ============================================================================
// Type Information
// ============================================================================

/// Information about a type definition
#[derive(Debug, Clone)]
pub struct TypeInfo {
    pub name: String,
    pub kind: TypeKind,
    pub span: Span,
    /// The interface this type is defined in (None for world-level types)
    pub interface: Option<String>,
}

/// Kind of type for resolution
#[derive(Debug, Clone)]
pub enum TypeKind {
    Primitive,
    Record {
        fields: HashMap<String, Ty>,
    },
    Variant {
        /// case name -> whether this case requires a payload
        cases: HashMap<String, bool>,
    },
    Enum {
        cases: Vec<String>,
    },
    Flags {
        flags: Vec<String>,
    },
    Resource,
    Alias {
        target: Box<Ty>,
        constraint: Option<Expr>,
    },
}

/// Information about an interface
#[derive(Debug, Clone)]
pub struct InterfaceInfo {
    pub name: String,
    pub span: Span,
    /// Types exported by this interface
    pub types: Vec<String>,
    /// Functions exported by this interface
    pub functions: Vec<String>,
    /// Return types by function name
    function_returns: HashMap<String, CheckedType>,
    /// Parameter names by function
    pub function_params: HashMap<String, Vec<String>>,
    /// Parameter constraints by function
    pub function_constraints: HashMap<String, Vec<ParamConstraint>>,
    /// Parameter types by function
    pub function_param_types: HashMap<String, Vec<String>>,
    /// Constraints inherited transitively from callees
    inherited_constraints: HashMap<String, Vec<InheritedConstraint>>,
    /// Return type constraints (from constrained type aliases on return types)
    pub return_constraints: HashMap<String, ParamConstraint>,
}

/// A parameter constraint from a `where` clause
#[derive(Debug, Clone)]
pub struct ParamConstraint {
    pub param_name: String,
    pub constraint: Expr,
}

/// How a target function's parameter is mapped from the intermediate function's scope
#[derive(Debug, Clone)]
enum ParamSource {
    /// References a parameter of the intermediate function
    Param(String),
    /// A constant value resolved during pre-computation
    Constant(i64),
}

/// A constraint inherited transitively from a callee's callee
#[derive(Debug, Clone)]
struct InheritedConstraint {
    /// The function that originally has the constraint
    target_func: String,
    /// The chain of intermediate function names
    via: Vec<String>,
    /// The constraint from the target function
    constraint: ParamConstraint,
    /// Maps target function param names to their source in the intermediate function
    target_param_sources: HashMap<String, ParamSource>,
}

/// Information about a world
#[derive(Debug, Clone)]
pub struct WorldInfo {
    pub name: String,
    pub span: Span,
    pub imports: Vec<String>,
    pub exports: Vec<String>,
}

/// Result of evaluating a constraint
#[derive(Debug, Clone)]
enum ConstraintEvalResult {
    /// Constraint is satisfied
    Satisfied,
    /// Constraint is violated with error message
    Violated(#[allow(dead_code)] String),
    /// Constraint needs propagation (has free variables)
    NeedsPropagation(Vec<String>),
}

/// Hush comment extracted from source for constraint checking
#[derive(Debug, Clone)]
struct HushComment {
    line: usize,
    col: usize,
    constraint: Expr,
    #[allow(dead_code)]
    span: Span,
}

// ============================================================================
// Checker Implementation
// ============================================================================

struct Checker {
    /// All type definitions (scoped by interface name or global)
    types: HashMap<TypeKey, TypeInfo>,
    /// All interface definitions
    interfaces: HashMap<String, InterfaceInfo>,
    /// All world definitions
    worlds: HashMap<String, WorldInfo>,
    /// Current scope for type resolution
    current_interface: Option<String>,
    /// Types currently being resolved (for recursion detection)
    resolving: HashSet<String>,
    /// Collected diagnostics
    diagnostics: Vec<Diagnostic>,
    /// Local variable scope (name -> type) for expression checking
    locals: HashMap<String, CheckedType>,
    /// Interface qualifiers imported by worlds in this check scope (e.g., `math`)
    imported_interface_bindings: HashSet<String>,
    /// Type parameters currently in scope (for generic type definitions)
    type_params: HashSet<String>,
    /// Nesting depth of loop bodies currently being validated
    loop_depth: usize,
    /// Tracked constant values for constraint evaluation
    constants: HashMap<String, i64>,
    /// Hush comments (constraint annotations) collected during parsing
    hush_comments: Vec<HushComment>,
    /// Whether currently checking inside a @test or @test-helper function
    in_test_function: bool,
    /// Declared return type of the current function being validated
    current_return_type: Option<CheckedType>,
    /// Whether the current expression is the value of a guard-let (skip constraint checks)
    in_guard_let: bool,
    /// Byte offsets of the start of each line in the source (for span-to-line conversion)
    source_line_offsets: Vec<usize>,
}

/// Checked type representation for expression type checking
#[derive(Debug, Clone, PartialEq)]
enum CheckedType {
    Bool,
    I32,
    I64,
    F32,
    F64,
    String,
    Named(String),
    Interface(String),
    List(Box<CheckedType>),
    Option(Box<CheckedType>),
    Result {
        ok: Option<Box<CheckedType>>,
        err: Option<Box<CheckedType>>,
    },
    Future(Option<Box<CheckedType>>),
    /// Opaque thread identifier returned by `spawn` — not numeric, no arithmetic allowed
    ThreadId,
    /// Opaque shared memory variable — used with `atomic { }` blocks
    Shared,
    /// 128-bit SIMD vector
    V128,
    Unknown,
}

/// Key for looking up types (interface-scoped or global)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TypeKey {
    interface: Option<String>,
    name: String,
}

impl Checker {
    fn new() -> Self {
        Self {
            types: HashMap::new(),
            interfaces: HashMap::new(),
            worlds: HashMap::new(),
            current_interface: None,
            resolving: HashSet::new(),
            diagnostics: Vec::new(),
            locals: HashMap::new(),
            imported_interface_bindings: HashSet::new(),
            type_params: HashSet::new(),
            loop_depth: 0,
            constants: HashMap::new(),
            hush_comments: Vec::new(),
            in_test_function: false,
            current_return_type: None,
            in_guard_let: false,
            source_line_offsets: Vec::new(),
        }
    }

    fn check_file(&mut self, file: &WitFile) {
        self.collect_definitions(file);
        self.validate_file(file);
    }

    // ========================================================================
    // Pass 1: Collect all definitions
    // ========================================================================

    fn collect_definitions(&mut self, file: &WitFile) {
        for item in &file.items {
            match item {
                TopLevelItem::Interface(iface) => self.collect_interface(iface),
                TopLevelItem::World(world) => self.collect_world(world),
                TopLevelItem::Use(_) => {} // Handled in validation
                TopLevelItem::NestedPackage(_) => {} // Nested packages are validated separately
            }
        }

        let builtin_types = [
            "bool", "s8", "s16", "s32", "s64", "u8", "u16", "u32", "u64", "f32", "f64", "char",
            "string", "list", "option",
        ];
        for name in builtin_types {
            let key = TypeKey {
                interface: None,
                name: name.to_string(),
            };
            if !self.types.contains_key(&key) {
                self.types.insert(
                    key,
                    TypeInfo {
                        name: name.to_string(),
                        kind: TypeKind::Primitive,
                        span: 0..0,
                        interface: None,
                    },
                );
            }
        }

        let result_key = TypeKey {
            interface: None,
            name: "result".to_string(),
        };
        if !self.types.contains_key(&result_key) {
            let mut cases = HashMap::new();
            cases.insert("ok".to_string(), true);
            cases.insert("err".to_string(), true);
            self.types.insert(
                result_key,
                TypeInfo {
                    name: "result".to_string(),
                    kind: TypeKind::Variant { cases },
                    span: 0..0,
                    interface: None,
                },
            );
        }
    }

    fn collect_interface(&mut self, iface: &Interface) {
        let iface_name = iface.name.name.clone();

        if self.interfaces.contains_key(&iface_name) {
            self.diagnostics.push(Diagnostic::error(
                format!("Duplicate interface definition: {}", iface_name),
                iface.span.clone(),
                DiagnosticCode::DuplicateDefinition,
            ));
            return;
        }

        self.current_interface = Some(iface_name.clone());

        let mut types = Vec::new();
        let mut functions = Vec::new();
        let mut function_returns = HashMap::new();
        let mut function_params = HashMap::new();
        let mut function_param_types = HashMap::new();
        let mut function_constraints = HashMap::new();

        for item in &iface.items {
            match item {
                InterfaceItem::TypeDef(typedef) => {
                    let name = self.collect_typedef(typedef, Some(&iface_name));
                    types.push(name);
                }
                InterfaceItem::Func(func) => {
                    let func_name = func.name.name.clone();
                    functions.push(func_name.clone());
                    if let Some(result) = &func.result {
                        function_returns.insert(func_name.clone(), self.ty_to_checked(result));
                    }
                    let param_names: Vec<String> =
                        func.params.iter().map(|p| p.name.name.clone()).collect();
                    if !param_names.is_empty() {
                        function_params.insert(func_name.clone(), param_names.clone());
                    }
                    let param_types: Vec<String> = func
                        .params
                        .iter()
                        .map(|p| format!("{}: {}", p.name.name, self.fmt_ty(&p.ty)))
                        .collect();
                    if !param_types.is_empty() {
                        function_param_types.insert(func_name.clone(), param_types);
                    }
                    let mut constraints: Vec<ParamConstraint> = func
                        .params
                        .iter()
                        .filter_map(|p| {
                            p.constraint.as_ref().map(|c| ParamConstraint {
                                param_name: p.name.name.clone(),
                                constraint: c.clone(),
                            })
                        })
                        .collect();

                    for p in &func.params {
                        if let Some(type_constraint) = self.resolve_type_alias_constraint(&p.ty) {
                            let substituted =
                                substitute_expr_ident(&type_constraint, "it", &p.name.name);
                            constraints.push(ParamConstraint {
                                param_name: p.name.name.clone(),
                                constraint: substituted,
                            });
                        }
                    }

                    if !constraints.is_empty() {
                        function_constraints.insert(func_name, constraints);
                    }
                }
                InterfaceItem::Use(_) => {} // Handled in validation
            }
        }

        let mut return_constraints: HashMap<String, ParamConstraint> = HashMap::new();
        for item in &iface.items {
            if let InterfaceItem::Func(func) = item {
                if let Some(result) = &func.result {
                    if let Some(type_constraint) = self.resolve_type_alias_constraint(result) {
                        let func_name = func.name.name.clone();
                        return_constraints.insert(
                            func_name,
                            ParamConstraint {
                                param_name: "result".to_string(),
                                constraint: type_constraint,
                            },
                        );
                    }
                }
            }
        }

        let inherited_constraints =
            self.compute_inherited_constraints(iface, &function_constraints, &function_params);

        self.interfaces.insert(
            iface_name.clone(),
            InterfaceInfo {
                name: iface_name,
                span: iface.span.clone(),
                types,
                functions,
                function_returns,
                function_params,
                function_constraints,
                function_param_types,
                inherited_constraints,
                return_constraints,
            },
        );

        self.current_interface = None;
    }

    fn collect_world(&mut self, world: &World) {
        let world_name = world.name.name.clone();

        // Check for duplicate world
        if self.worlds.contains_key(&world_name) {
            self.diagnostics.push(Diagnostic::error(
                format!("Duplicate world definition: {}", world_name),
                world.span.clone(),
                DiagnosticCode::DuplicateDefinition,
            ));
            return;
        }

        let mut imports = Vec::new();
        let mut exports = Vec::new();

        for item in &world.items {
            match item {
                WorldItem::Import(ie) => match &ie.kind {
                    ImportExportKind::Path(path) => {
                        imports.push(path.interface.name.clone());
                        self.imported_interface_bindings
                            .insert(path.interface.name.clone());
                    }
                    _ => {
                        if let Some(name) = &ie.name {
                            imports.push(name.name.clone());
                        }
                    }
                },
                WorldItem::Export(ie) => {
                    if let Some(name) = &ie.name {
                        exports.push(name.name.clone());
                    }
                }
                WorldItem::TypeDef(typedef) => {
                    self.collect_typedef(typedef, None);
                }
                _ => {}
            }
        }

        self.worlds.insert(
            world_name.clone(),
            WorldInfo {
                name: world_name,
                span: world.span.clone(),
                imports,
                exports,
            },
        );
    }

    fn collect_typedef(&mut self, typedef: &TypeDef, interface: Option<&str>) -> String {
        let name = typedef_name(&typedef.kind);
        let key = TypeKey {
            interface: interface.map(String::from),
            name: name.clone(),
        };

        // Check for duplicate type in same scope
        if self.types.contains_key(&key) {
            self.diagnostics.push(Diagnostic::error(
                format!("Duplicate type definition: {}", name),
                typedef.span.clone(),
                DiagnosticCode::DuplicateDefinition,
            ));
        } else {
            self.types.insert(
                key,
                TypeInfo {
                    name: name.clone(),
                    kind: typedef_to_kind(&typedef.kind),
                    span: typedef.span.clone(),
                    interface: interface.map(String::from),
                },
            );
        }

        name
    }

    // ========================================================================
    // Pass 2: Validate all references
    // ========================================================================

    fn validate_file(&mut self, file: &WitFile) {
        for item in &file.items {
            match item {
                TopLevelItem::Interface(iface) => self.validate_interface(iface),
                TopLevelItem::World(world) => self.validate_world(world),
                TopLevelItem::Use(use_stmt) => self.validate_top_level_use(use_stmt),
                TopLevelItem::NestedPackage(pkg) => {
                    self.diagnostics.push(Diagnostic::error(
                        format!(
                            "Nested packages are not supported (found '{}')",
                            package_path_to_string(&pkg.path)
                        ),
                        pkg.span.clone(),
                        DiagnosticCode::NestedPackageUnsupported,
                    ));
                }
            }
        }
    }

    fn validate_interface(&mut self, iface: &Interface) {
        self.current_interface = Some(iface.name.name.clone());

        for item in &iface.items {
            match item {
                InterfaceItem::TypeDef(typedef) => self.validate_typedef(typedef),
                InterfaceItem::Func(func) => self.validate_func(func),
                InterfaceItem::Use(use_stmt) => self.validate_use_statement(use_stmt),
            }
        }

        self.current_interface = None;
    }

    fn validate_world(&mut self, world: &World) {
        for item in &world.items {
            match item {
                WorldItem::TypeDef(typedef) => self.validate_typedef(typedef),
                WorldItem::Import(ie) | WorldItem::Export(ie) => {
                    self.validate_import_export(ie);
                }
                WorldItem::Use(use_stmt) => self.validate_use_statement(use_stmt),
                WorldItem::Include(include) => self.validate_include(include),
            }
        }
    }

    fn validate_typedef(&mut self, typedef: &TypeDef) {
        let name = typedef_name(&typedef.kind);

        // Check for recursive types
        if self.resolving.contains(&name) {
            self.diagnostics.push(Diagnostic::error(
                format!("Recursive type definition: {}", name),
                typedef.span.clone(),
                DiagnosticCode::RecursiveType,
            ));
            return;
        }

        self.resolving.insert(name.clone());

        // Extract and add type parameters to scope before validating
        let type_params_to_add: Vec<String> = match &typedef.kind {
            TypeDefKind::Alias { type_params, .. }
            | TypeDefKind::Record { type_params, .. }
            | TypeDefKind::Variant { type_params, .. } => {
                type_params.iter().map(|id| id.name.clone()).collect()
            }
            _ => vec![],
        };
        for param in &type_params_to_add {
            self.type_params.insert(param.clone());
        }

        match &typedef.kind {
            TypeDefKind::Alias { ty, .. } => {
                self.validate_type(ty);
            }
            TypeDefKind::Record { fields, .. } => {
                for field in fields {
                    self.validate_type(&field.ty);
                }
            }
            TypeDefKind::Variant { cases, .. } => {
                for case in cases {
                    if let Some(ty) = &case.ty {
                        self.validate_type(ty);
                    }
                }
            }
            TypeDefKind::Enum { .. } | TypeDefKind::Flags { .. } => {
                // No type references to validate
            }
            TypeDefKind::Resource { methods, .. } => {
                for method in methods {
                    match method {
                        ResourceMethod::Method(f) => {
                            // Instance methods have implicit self parameter
                            self.validate_func_with_self(f);
                        }
                        ResourceMethod::Static(f) => {
                            self.validate_func(f);
                        }
                        ResourceMethod::Constructor {
                            params,
                            result,
                            body,
                            span: _,
                        } => {
                            for param in params {
                                self.validate_type(&param.ty);
                            }
                            if let Some(ty) = result {
                                self.validate_type(ty);
                            }
                            // Validate constructor body if present
                            if let Some(func_body) = body {
                                self.locals.clear();
                                for param in params {
                                    let checked_ty = self.ty_to_checked(&param.ty);
                                    self.locals.insert(param.name.name.clone(), checked_ty);
                                }
                                for stmt in &func_body.statements {
                                    self.validate_statement(stmt);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Remove type parameters from scope
        for param in &type_params_to_add {
            self.type_params.remove(param);
        }

        self.resolving.remove(&name);
    }

    fn validate_func(&mut self, func: &Func) {
        // Validate parameter types
        for param in &func.params {
            self.validate_type(&param.ty);
        }

        let has_constraints = func.params.iter().any(|p| p.constraint.is_some());
        if has_constraints {
            let is_result = match &func.result {
                Some(ty) => matches!(ty, Ty::Result { .. }),
                None => false,
            };
            if !is_result {
                self.diagnostics.push(Diagnostic::error(
                    format!(
                        "Function '{}' with parameter constraints must return result",
                        func.name.name
                    ),
                    func.name.span.clone(),
                    DiagnosticCode::ConstraintViolation,
                ));
            }
        }

        // Validate result type
        let declared_return = if let Some(ty) = &func.result {
            self.validate_type(ty);
            Some(self.ty_to_checked(ty))
        } else {
            None
        };

        // Validate function body (Kettu extension)
        if let Some(body) = &func.body {
            // Check if this is a test or test-helper function
            let was_in_test = self.in_test_function;
            self.in_test_function = func
                .gates
                .iter()
                .any(|g| matches!(g, Gate::Test | Gate::TestHelper));

            let was_return_type = self.current_return_type.clone();
            self.current_return_type = declared_return.clone();

            // Clear locals and add parameters
            self.locals.clear();
            self.constants.clear();
            for interface_name in &self.imported_interface_bindings {
                self.locals.insert(
                    interface_name.clone(),
                    CheckedType::Interface(interface_name.clone()),
                );
            }
            for param in &func.params {
                let checked_ty = self.ty_to_checked(&param.ty);
                self.locals.insert(param.name.name.clone(), checked_ty);
            }

            // Validate each statement and track inferred return type
            let mut inferred_return = CheckedType::Unknown;
            for (i, stmt) in body.statements.iter().enumerate() {
                let is_last = i == body.statements.len() - 1;

                match stmt {
                    Statement::Return(Some(expr)) => {
                        let return_ty = self.check_expr(expr);
                        // Compare with declared return type
                        if let Some(ref declared) = declared_return {
                            if return_ty != *declared
                                && return_ty != CheckedType::Unknown
                                && *declared != CheckedType::Unknown
                            {
                                self.diagnostics.push(Diagnostic::error(
                                    format!(
                                        "Return type mismatch: expected {:?}, found {:?}",
                                        declared, return_ty
                                    ),
                                    func.name.span.clone(),
                                    DiagnosticCode::TypeMismatch,
                                ));
                            }
                        }
                        inferred_return = return_ty;
                    }
                    Statement::Return(None) => {
                        // Void return - if function has return type, that's an error
                        if declared_return.is_some() {
                            self.diagnostics.push(Diagnostic::error(
                                "Missing return value".to_string(),
                                func.name.span.clone(),
                                DiagnosticCode::TypeMismatch,
                            ));
                        }
                    }
                    Statement::Expr(expr) if is_last => {
                        // Last expression is implicit return
                        let expr_ty = self.check_expr(expr);
                        if let Some(ref declared) = declared_return {
                            if expr_ty != *declared
                                && expr_ty != CheckedType::Unknown
                                && *declared != CheckedType::Unknown
                            {
                                self.diagnostics.push(Diagnostic::error(
                                    format!(
                                        "Return type mismatch: expected {:?}, found {:?}",
                                        declared, expr_ty
                                    ),
                                    func.name.span.clone(),
                                    DiagnosticCode::TypeMismatch,
                                ));
                            }
                        }
                        inferred_return = expr_ty;
                    }
                    _ => {
                        self.validate_statement(stmt);
                    }
                }
            }

            // If function declares return type but body has no return/expr, that's an error
            if declared_return.is_some()
                && inferred_return == CheckedType::Unknown
                && !body.statements.is_empty()
            {
                // Only warn if the last statement isn't already a return
                if !matches!(body.statements.last(), Some(Statement::Return(_))) {
                    self.diagnostics.push(Diagnostic::warning(
                        "Function may not return a value".to_string(),
                        func.name.span.clone(),
                        DiagnosticCode::TypeMismatch,
                    ));
                }
            }

            // Restore previous test function state after body processing
            self.in_test_function = was_in_test;
            self.current_return_type = was_return_type;
        }

        // Check feature gates
        for gate in &func.gates {
            self.validate_gate(gate, func.name.span.clone());
        }
    }

    /// Validate a function with implicit self parameter (for resource instance methods)
    fn validate_func_with_self(&mut self, func: &Func) {
        // Validate parameter types
        for param in &func.params {
            self.validate_type(&param.ty);
        }

        // Validate result type
        let declared_return = if let Some(ty) = &func.result {
            self.validate_type(ty);
            Some(self.ty_to_checked(ty))
        } else {
            None
        };

        // Validate function body (Kettu extension)
        if let Some(body) = &func.body {
            // Check if this is a test or test-helper function
            let was_in_test = self.in_test_function;
            self.in_test_function = func
                .gates
                .iter()
                .any(|g| matches!(g, Gate::Test | Gate::TestHelper));

            let was_return_type = self.current_return_type.clone();
            self.current_return_type = declared_return.clone();

            // Clear locals and add implicit self + explicit parameters
            self.locals.clear();
            for interface_name in &self.imported_interface_bindings {
                self.locals.insert(
                    interface_name.clone(),
                    CheckedType::Interface(interface_name.clone()),
                );
            }
            self.locals.insert("self".to_string(), CheckedType::I32);
            for param in &func.params {
                let checked_ty = self.ty_to_checked(&param.ty);
                self.locals.insert(param.name.name.clone(), checked_ty);
            }

            // Validate each statement
            for stmt in &body.statements {
                self.validate_statement(stmt);
            }

            // Check last statement for return (simplified - same as validate_func)
            if let Some(Statement::Return(Some(expr))) = body.statements.last() {
                let return_ty = self.check_expr(expr);
                if let Some(ref declared) = declared_return {
                    if return_ty != *declared
                        && return_ty != CheckedType::Unknown
                        && *declared != CheckedType::Unknown
                    {
                        self.diagnostics.push(Diagnostic::error(
                            format!(
                                "Return type mismatch: expected {:?}, found {:?}",
                                declared, return_ty
                            ),
                            func.name.span.clone(),
                            DiagnosticCode::TypeMismatch,
                        ));
                    }
                }
            }

            // Restore previous test function state after body processing
            self.in_test_function = was_in_test;
            self.current_return_type = was_return_type;
        }

        // Check feature gates
        for gate in &func.gates {
            self.validate_gate(gate, func.name.span.clone());
        }
    }

    fn validate_gate(&mut self, gate: &Gate, span: Span) {
        match gate {
            Gate::Deprecated { version } => {
                self.diagnostics.push(Diagnostic::warning(
                    format!(
                        "Deprecated since version {}.{}.{}",
                        version.major, version.minor, version.patch
                    ),
                    span,
                    DiagnosticCode::DeprecatedFeature,
                ));
            }
            Gate::Unstable { feature } => {
                self.diagnostics.push(Diagnostic {
                    message: format!("Unstable feature: {}", feature.name),
                    span,
                    severity: Severity::Info,
                    code: DiagnosticCode::UnstableFeature,
                });
            }
            Gate::Since { .. } => {
                // No warning for @since
            }
            Gate::Test => {
                // Test functions are validated separately
            }
            Gate::TestHelper => {
                // Test helper functions are validated separately
            }
        }
    }

    fn validate_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Let { name, value } => {
                // Check the value expression
                let value_ty = self.check_expr(value);
                // Add to local scope (infer type from value)
                self.locals.insert(name.name.clone(), value_ty);
                // Track constant values for constraint evaluation
                if let Some(val) = self.const_value(value) {
                    self.constants.insert(name.name.clone(), val);
                }
            }
            Statement::Return(value) => {
                if let Some(expr) = value {
                    self.check_expr(expr);
                }
            }
            Statement::Expr(expr) => {
                self.check_expr(expr);
            }
            Statement::Assign { name, value } => {
                // Check that the variable exists
                if !self.locals.contains_key(&name.name) {
                    self.diagnostics.push(Diagnostic::error(
                        format!("Cannot assign to undefined variable: {}", name.name),
                        name.span.clone(),
                        DiagnosticCode::UnknownVariable,
                    ));
                }
                // Check the value expression
                self.check_expr(value);
            }
            Statement::CompoundAssign { name, value, .. } => {
                if !self.locals.contains_key(&name.name) {
                    self.diagnostics.push(Diagnostic::error(
                        format!("Cannot assign to undefined variable: {}", name.name),
                        name.span.clone(),
                        DiagnosticCode::UnknownVariable,
                    ));
                }
                self.check_expr(value);
            }
            Statement::Break { condition } | Statement::Continue { condition } => {
                // Check condition if present (should be bool)
                if let Some(cond) = condition {
                    self.check_expr(cond);
                }
                if self.loop_depth == 0 {
                    let keyword = if matches!(stmt, Statement::Break { .. }) {
                        "break"
                    } else {
                        "continue"
                    };
                    let span = condition
                        .as_deref()
                        .map(Self::expr_span)
                        .unwrap_or_default();
                    self.diagnostics.push(Diagnostic::error(
                        format!("`{keyword}` can only appear inside a loop"),
                        span,
                        DiagnosticCode::InvalidUse,
                    ));
                }
            }
            Statement::SharedLet {
                name,
                initial_value,
            } => {
                let _value_ty = self.check_expr(initial_value);
                // Shared variables always have type Shared (opaque)
                self.locals.insert(name.name.clone(), CheckedType::Shared);
            }
            Statement::GuardLet {
                name,
                value,
                else_body,
            } => {
                self.in_guard_let = true;
                let value_ty = self.check_expr(value);
                self.in_guard_let = false;
                let binding_ty = self.guard_binding_type(value_ty, Self::expr_span(value));
                self.validate_guard_else_body(
                    else_body,
                    name.span.clone(),
                    "`guard let` else block must exit the current scope with `return`, `break`, or `continue`",
                );
                self.locals.insert(name.name.clone(), binding_ty);
            }
            Statement::Guard {
                condition,
                else_body,
            } => {
                let cond_ty = self.check_expr(condition);
                if cond_ty != CheckedType::Bool && cond_ty != CheckedType::Unknown {
                    self.diagnostics.push(Diagnostic::error(
                        format!("Guard condition requires bool, got {:?}", cond_ty),
                        Self::expr_span(condition),
                        DiagnosticCode::TypeMismatch,
                    ));
                }

                self.validate_guard_else_body(
                    else_body,
                    Self::expr_span(condition),
                    "`guard` else block must exit the current scope with `return`, `break`, or `continue`",
                );
            }
        }
    }

    fn validate_block(&mut self, statements: &[Statement]) {
        for stmt in statements {
            self.validate_statement(stmt);
        }
    }

    fn check_block_tail_type(&mut self, statements: &[Statement]) -> CheckedType {
        let mut tail = CheckedType::Unknown;

        for (i, stmt) in statements.iter().enumerate() {
            let is_last = i == statements.len() - 1;

            match stmt {
                Statement::Return(Some(expr)) => {
                    tail = self.check_expr(expr);
                }
                Statement::Return(None) => {
                    tail = CheckedType::Unknown;
                }
                Statement::Expr(expr) if is_last => {
                    tail = self.check_expr(expr);
                }
                _ => {
                    self.validate_statement(stmt);
                }
            }
        }

        tail
    }

    fn guard_binding_type(&mut self, value_ty: CheckedType, span: Span) -> CheckedType {
        match value_ty {
            CheckedType::Option(inner) => *inner,
            CheckedType::Result {
                ok: Some(inner), ..
            } => *inner,
            CheckedType::Unknown => CheckedType::Unknown,
            CheckedType::Result { ok: None, .. } => {
                self.diagnostics.push(Diagnostic::error(
                    "Guard let requires option<T> or result<T, E> with an `ok` payload".to_string(),
                    span,
                    DiagnosticCode::TypeMismatch,
                ));
                CheckedType::Unknown
            }
            other => {
                self.diagnostics.push(Diagnostic::error(
                    format!(
                        "Guard let requires option<T> or result<T, E>, got {:?}",
                        other
                    ),
                    span,
                    DiagnosticCode::TypeMismatch,
                ));
                CheckedType::Unknown
            }
        }
    }

    fn validate_guard_else_body(&mut self, else_body: &[Statement], span: Span, message: &str) {
        let saved_locals = self.locals.clone();
        self.validate_block(else_body);
        self.locals = saved_locals;

        if !self.block_exits_scope(else_body) {
            self.diagnostics.push(Diagnostic::error(
                message.to_string(),
                span,
                DiagnosticCode::InvalidUse,
            ));
        }
    }

    fn block_exits_scope(&self, statements: &[Statement]) -> bool {
        match statements.last() {
            Some(stmt) => self.statement_exits_scope(stmt),
            None => false,
        }
    }

    fn statement_exits_scope(&self, stmt: &Statement) -> bool {
        match stmt {
            Statement::Return(_) => true,
            Statement::Break { .. } | Statement::Continue { .. } => self.loop_depth > 0,
            Statement::Expr(Expr::If {
                then_branch,
                else_branch: Some(else_branch),
                ..
            }) => self.block_exits_scope(then_branch) && self.block_exits_scope(else_branch),
            _ => false,
        }
    }

    fn expr_span(expr: &Expr) -> Span {
        match expr {
            Expr::Ident(id) => id.span.clone(),
            Expr::Integer(_, span)
            | Expr::String(_, span)
            | Expr::InterpolatedString(_, span)
            | Expr::Bool(_, span)
            | Expr::Assert(_, span)
            | Expr::Not(_, span)
            | Expr::Neg(_, span)
            | Expr::StrLen(_, span)
            | Expr::ListLen(_, span)
            | Expr::StrEq(_, _, span)
            | Expr::ListSet(_, _, _, span)
            | Expr::ListPush(_, _, span) => span.clone(),
            Expr::Call { span, .. }
            | Expr::Field { span, .. }
            | Expr::OptionalChain { span, .. }
            | Expr::Try { span, .. }
            | Expr::Binary { span, .. }
            | Expr::If { span, .. }
            | Expr::Lambda { span, .. }
            | Expr::Map { span, .. }
            | Expr::Filter { span, .. }
            | Expr::Reduce { span, .. }
            | Expr::RecordLiteral { span, .. }
            | Expr::VariantLiteral { span, .. }
            | Expr::Match { span, .. }
            | Expr::While { span, .. }
            | Expr::Range { span, .. }
            | Expr::For { span, .. }
            | Expr::ListLiteral { span, .. }
            | Expr::Index { span, .. }
            | Expr::Slice { span, .. }
            | Expr::ForEach { span, .. }
            | Expr::Await { span, .. }
            | Expr::AtomicLoad { span, .. }
            | Expr::AtomicStore { span, .. }
            | Expr::AtomicAdd { span, .. }
            | Expr::AtomicSub { span, .. }
            | Expr::AtomicCmpxchg { span, .. }
            | Expr::AtomicWait { span, .. }
            | Expr::AtomicNotify { span, .. }
            | Expr::Spawn { span, .. }
            | Expr::ThreadJoin { span, .. }
            | Expr::AtomicBlock { span, .. }
            | Expr::SimdOp { span, .. }
            | Expr::SimdForEach { span, .. } => span.clone(),
        }
    }

    fn check_expr(&mut self, expr: &Expr) -> CheckedType {
        match expr {
            Expr::Integer(_, _) => CheckedType::I32,
            Expr::String(_, _) => CheckedType::String,
            Expr::InterpolatedString(parts, _) => {
                // Type check each expression part
                for part in parts {
                    if let StringPart::Expr(expr) = part {
                        self.check_expr(expr);
                        // We allow any type in interpolation - it will be converted to string at runtime
                    }
                }
                CheckedType::String
            }
            Expr::Bool(_, _) => CheckedType::Bool,
            Expr::Ident(id) => {
                if let Some(ty) = self.locals.get(&id.name) {
                    ty.clone()
                } else {
                    self.diagnostics.push(Diagnostic::error(
                        format!("Unknown variable: {}", id.name),
                        id.span.clone(),
                        DiagnosticCode::UnknownVariable,
                    ));
                    CheckedType::Unknown
                }
            }
            Expr::Binary { lhs, op, rhs, span } => {
                let lhs_ty = self.check_expr(lhs);
                let rhs_ty = self.check_expr(rhs);

                use kettu_parser::BinOp;
                match op {
                    BinOp::Add => {
                        // Add works on numeric types OR strings (concatenation)
                        if lhs_ty == CheckedType::String && rhs_ty == CheckedType::String {
                            CheckedType::String
                        } else if !self.is_numeric(&lhs_ty) {
                            self.diagnostics.push(Diagnostic::error(
                                format!(
                                    "Operator {:?} requires numeric or string type, got {:?}",
                                    op, lhs_ty
                                ),
                                span.clone(),
                                DiagnosticCode::InvalidOperator,
                            ));
                            lhs_ty
                        } else {
                            lhs_ty
                        }
                    }
                    BinOp::Sub | BinOp::Mul | BinOp::Div => {
                        // Arithmetic operators require numeric types
                        if !self.is_numeric(&lhs_ty) {
                            self.diagnostics.push(Diagnostic::error(
                                format!(
                                    "Operator {:?} requires numeric type, got {:?}",
                                    op, lhs_ty
                                ),
                                span.clone(),
                                DiagnosticCode::InvalidOperator,
                            ));
                        }
                        lhs_ty
                    }
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                        // Comparison operators return bool
                        CheckedType::Bool
                    }
                    BinOp::And | BinOp::Or => {
                        // Logical operators require bool
                        if lhs_ty != CheckedType::Bool {
                            self.diagnostics.push(Diagnostic::error(
                                format!("Operator {:?} requires bool, got {:?}", op, lhs_ty),
                                span.clone(),
                                DiagnosticCode::InvalidOperator,
                            ));
                        }
                        if rhs_ty != CheckedType::Bool {
                            self.diagnostics.push(Diagnostic::error(
                                format!("Operator {:?} requires bool, got {:?}", op, rhs_ty),
                                span.clone(),
                                DiagnosticCode::InvalidOperator,
                            ));
                        }
                        CheckedType::Bool
                    }
                }
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
                span,
            } => {
                let cond_ty = self.check_expr(cond);
                if cond_ty != CheckedType::Bool && cond_ty != CheckedType::Unknown {
                    self.diagnostics.push(Diagnostic::error(
                        format!("If condition requires bool, got {:?}", cond_ty),
                        span.clone(),
                        DiagnosticCode::TypeMismatch,
                    ));
                }

                let saved_locals = self.locals.clone();

                self.locals = saved_locals.clone();
                let then_ty = self.check_block_tail_type(then_branch);

                let else_ty = if let Some(else_stmts) = else_branch {
                    self.locals = saved_locals.clone();
                    Some(self.check_block_tail_type(else_stmts))
                } else {
                    None
                };

                self.locals = saved_locals;

                match else_ty {
                    Some(else_ty)
                        if then_ty != CheckedType::Unknown
                            && else_ty != CheckedType::Unknown
                            && then_ty != else_ty =>
                    {
                        self.diagnostics.push(Diagnostic::error(
                            format!(
                                "If branch type mismatch: then is {:?}, else is {:?}",
                                then_ty, else_ty
                            ),
                            span.clone(),
                            DiagnosticCode::TypeMismatch,
                        ));
                        CheckedType::Unknown
                    }
                    Some(else_ty) if then_ty == else_ty => then_ty,
                    _ => CheckedType::Unknown,
                }
            }
            Expr::Call { func, args, span } => {
                let callee_return = match func.as_ref() {
                    Expr::Ident(id) => {
                        if let Some(local) = self.locals.get(&id.name) {
                            local.clone()
                        } else if let Some(iface_name) = &self.current_interface {
                            self.interfaces
                                .get(iface_name)
                                .and_then(|iface| iface.function_returns.get(&id.name))
                                .cloned()
                                .unwrap_or_else(|| {
                                    self.diagnostics.push(Diagnostic::error(
                                        format!("Unknown variable: {}", id.name),
                                        id.span.clone(),
                                        DiagnosticCode::UnknownVariable,
                                    ));
                                    CheckedType::Unknown
                                })
                        } else {
                            self.diagnostics.push(Diagnostic::error(
                                format!("Unknown variable: {}", id.name),
                                id.span.clone(),
                                DiagnosticCode::UnknownVariable,
                            ));
                            CheckedType::Unknown
                        }
                    }
                    Expr::Field { expr, field, .. } => {
                        self.check_expr(func);
                        if let Expr::Ident(iface_id) = expr.as_ref() {
                            self.interfaces
                                .get(&iface_id.name)
                                .and_then(|iface| iface.function_returns.get(&field.name))
                                .cloned()
                                .unwrap_or(CheckedType::Unknown)
                        } else {
                            CheckedType::Unknown
                        }
                    }
                    _ => {
                        self.check_expr(func);
                        CheckedType::Unknown
                    }
                };

                for arg in args {
                    self.check_expr(arg);
                }

                // Check call constraints after checking args
                if let Expr::Ident(id) = func.as_ref() {
                    self.check_call_constraints(id, args, span);
                }

                callee_return
            }
            Expr::Field { expr, field, span } => {
                let expr_ty = self.check_expr(expr);
                if let CheckedType::Interface(iface_name) = &expr_ty {
                    if let Some(iface) = self.interfaces.get(iface_name) {
                        if !iface.functions.contains(&field.name)
                            && !iface.types.contains(&field.name)
                        {
                            self.diagnostics.push(Diagnostic::error(
                                format!(
                                    "Interface '{}' does not export '{}'",
                                    iface_name, field.name
                                ),
                                span.clone(),
                                DiagnosticCode::UnknownVariable,
                            ));
                        }
                    }
                    return CheckedType::Unknown;
                }

                if let CheckedType::Named(type_name) = &expr_ty {
                    let type_kind = self.resolve_type_info(type_name).map(|i| i.kind.clone());
                    if let Some(TypeKind::Record { fields }) = type_kind {
                        if let Some(field_ty) = fields.get(&field.name) {
                            return self.ty_to_checked(field_ty);
                        }

                        self.diagnostics.push(Diagnostic::error(
                            format!("Record type '{}' has no field '{}'", type_name, field.name),
                            span.clone(),
                            DiagnosticCode::UnknownVariable,
                        ));
                        return CheckedType::Unknown;
                    }
                }
                CheckedType::Unknown
            }
            Expr::Assert(cond, span) => {
                let cond_ty = self.check_expr(cond);
                if cond_ty != CheckedType::Bool && cond_ty != CheckedType::Unknown {
                    self.diagnostics.push(Diagnostic::error(
                        format!("Assert requires bool condition, got {:?}", cond_ty),
                        span.clone(),
                        DiagnosticCode::TypeMismatch,
                    ));
                }
                CheckedType::Bool // Assert evaluates to true if it passes
            }
            Expr::Not(expr, span) => {
                let expr_ty = self.check_expr(expr);
                if expr_ty != CheckedType::Bool && expr_ty != CheckedType::Unknown {
                    self.diagnostics.push(Diagnostic::error(
                        format!("Negation requires bool, got {:?}", expr_ty),
                        span.clone(),
                        DiagnosticCode::TypeMismatch,
                    ));
                }
                CheckedType::Bool
            }
            Expr::Neg(expr, span) => {
                let expr_ty = self.check_expr(expr);
                if !self.is_numeric(&expr_ty) && expr_ty != CheckedType::Unknown {
                    self.diagnostics.push(Diagnostic::error(
                        format!("Unary minus requires numeric type, got {:?}", expr_ty),
                        span.clone(),
                        DiagnosticCode::TypeMismatch,
                    ));
                }
                CheckedType::I32
            }
            Expr::StrLen(expr, span) => {
                let expr_ty = self.check_expr(expr);
                if expr_ty != CheckedType::String && expr_ty != CheckedType::Unknown {
                    self.diagnostics.push(Diagnostic::error(
                        format!("str-len requires string, got {:?}", expr_ty),
                        span.clone(),
                        DiagnosticCode::TypeMismatch,
                    ));
                }
                CheckedType::I32
            }
            Expr::StrEq(a, b, span) => {
                let a_ty = self.check_expr(a);
                let b_ty = self.check_expr(b);
                if a_ty != CheckedType::String && a_ty != CheckedType::Unknown {
                    self.diagnostics.push(Diagnostic::error(
                        format!("str-eq first argument requires string, got {:?}", a_ty),
                        span.clone(),
                        DiagnosticCode::TypeMismatch,
                    ));
                }
                if b_ty != CheckedType::String && b_ty != CheckedType::Unknown {
                    self.diagnostics.push(Diagnostic::error(
                        format!("str-eq second argument requires string, got {:?}", b_ty),
                        span.clone(),
                        DiagnosticCode::TypeMismatch,
                    ));
                }
                CheckedType::Bool
            }
            Expr::ListLen(expr, span) => {
                let expr_ty = self.check_expr(expr);
                match expr_ty {
                    CheckedType::List(_) | CheckedType::Unknown => {}
                    _ => {
                        self.diagnostics.push(Diagnostic::error(
                            format!("list-len requires list, got {:?}", expr_ty),
                            span.clone(),
                            DiagnosticCode::TypeMismatch,
                        ));
                    }
                }
                CheckedType::I32
            }
            Expr::ListSet(arr_expr, idx_expr, val_expr, span) => {
                // Type check arr, idx, val
                let arr_ty = self.check_expr(arr_expr);
                let idx_ty = self.check_expr(idx_expr);
                let val_ty = self.check_expr(val_expr);

                // arr must be a list
                let elem_ty = match &arr_ty {
                    CheckedType::List(inner) => *inner.clone(),
                    CheckedType::Unknown => CheckedType::Unknown,
                    _ => {
                        self.diagnostics.push(Diagnostic::error(
                            format!("list-set requires list, got {:?}", arr_ty),
                            span.clone(),
                            DiagnosticCode::TypeMismatch,
                        ));
                        CheckedType::Unknown
                    }
                };

                // idx must be i32
                if idx_ty != CheckedType::I32 && idx_ty != CheckedType::Unknown {
                    self.diagnostics.push(Diagnostic::error(
                        format!("list-set index must be i32, got {:?}", idx_ty),
                        span.clone(),
                        DiagnosticCode::TypeMismatch,
                    ));
                }

                // val must match element type
                if elem_ty != CheckedType::Unknown
                    && val_ty != CheckedType::Unknown
                    && elem_ty != val_ty
                {
                    self.diagnostics.push(Diagnostic::error(
                        format!(
                            "list-set value type {:?} doesn't match element type {:?}",
                            val_ty, elem_ty
                        ),
                        span.clone(),
                        DiagnosticCode::TypeMismatch,
                    ));
                }

                // Returns the list type (for chaining)
                arr_ty
            }
            Expr::ListPush(arr_expr, val_expr, span) => {
                // Type check arr and val
                let arr_ty = self.check_expr(arr_expr);
                let val_ty = self.check_expr(val_expr);

                // arr must be a list
                let elem_ty = match &arr_ty {
                    CheckedType::List(inner) => *inner.clone(),
                    CheckedType::Unknown => CheckedType::Unknown,
                    _ => {
                        self.diagnostics.push(Diagnostic::error(
                            format!("list-push requires list, got {:?}", arr_ty),
                            span.clone(),
                            DiagnosticCode::TypeMismatch,
                        ));
                        CheckedType::Unknown
                    }
                };

                // val must match element type
                if elem_ty != CheckedType::Unknown
                    && val_ty != CheckedType::Unknown
                    && elem_ty != val_ty
                {
                    self.diagnostics.push(Diagnostic::error(
                        format!(
                            "list-push value type {:?} doesn't match element type {:?}",
                            val_ty, elem_ty
                        ),
                        span.clone(),
                        DiagnosticCode::TypeMismatch,
                    ));
                }

                // Returns the same list type (new list with element appended)
                arr_ty
            }
            Expr::Lambda { params, body, .. } => {
                // Create a temporary scope with lambda params
                let prev_locals = self.locals.clone();
                for param in params {
                    // For now, assume params are i32 - proper type inference would be needed
                    self.locals.insert(param.name.clone(), CheckedType::I32);
                }
                // Type check body
                let _body_ty = self.check_expr(body);
                // Restore scope
                self.locals = prev_locals;
                // Return Unknown for now - function types not yet fully supported
                CheckedType::Unknown
            }
            Expr::Map { list, lambda, span } => {
                // Type check the list expression
                let list_ty = self.check_expr(list);
                let elem_ty = match &list_ty {
                    CheckedType::List(inner) => *inner.clone(),
                    CheckedType::Unknown => CheckedType::Unknown,
                    _ => {
                        self.diagnostics.push(Diagnostic::error(
                            format!("map requires a list, got {:?}", list_ty),
                            span.clone(),
                            DiagnosticCode::TypeMismatch,
                        ));
                        CheckedType::Unknown
                    }
                };

                // Type check the lambda
                if let Expr::Lambda { params, body, .. } = lambda.as_ref() {
                    // Create scope with lambda param bound to element type
                    let prev_locals = self.locals.clone();
                    if !params.is_empty() {
                        self.locals.insert(params[0].name.clone(), elem_ty.clone());
                    }
                    let body_ty = self.check_expr(body);
                    self.locals = prev_locals;
                    // Result is a list of the body type
                    CheckedType::List(Box::new(body_ty))
                } else if let Expr::Ident(id) = lambda.as_ref() {
                    // Function variable - accept it if it exists in scope
                    if self.locals.get(&id.name).is_none() {
                        self.diagnostics.push(Diagnostic::error(
                            format!("unknown function variable: {}", id.name),
                            span.clone(),
                            DiagnosticCode::TypeMismatch,
                        ));
                    }
                    // Result is a list (type inference could be improved)
                    CheckedType::List(Box::new(elem_ty))
                } else {
                    self.diagnostics.push(Diagnostic::error(
                        "map requires a lambda expression as second argument".to_string(),
                        span.clone(),
                        DiagnosticCode::TypeMismatch,
                    ));
                    list_ty
                }
            }
            Expr::Filter { list, lambda, span } => {
                // Same as Map but lambda must return Bool and result is same list type
                let list_ty = self.check_expr(list);
                let elem_ty = match &list_ty {
                    CheckedType::List(inner) => *inner.clone(),
                    CheckedType::Unknown => CheckedType::Unknown,
                    _ => {
                        self.diagnostics.push(Diagnostic::error(
                            format!("filter requires a list, got {:?}", list_ty),
                            span.clone(),
                            DiagnosticCode::TypeMismatch,
                        ));
                        CheckedType::Unknown
                    }
                };

                if let Expr::Lambda { params, body, .. } = lambda.as_ref() {
                    let prev_locals = self.locals.clone();
                    if !params.is_empty() {
                        self.locals.insert(params[0].name.clone(), elem_ty);
                    }
                    let _body_ty = self.check_expr(body);
                    self.locals = prev_locals;
                    // Returns list of same element type (filtered)
                    list_ty
                } else if let Expr::Ident(id) = lambda.as_ref() {
                    // Function variable - accept it if it exists in scope
                    if self.locals.get(&id.name).is_none() {
                        self.diagnostics.push(Diagnostic::error(
                            format!("unknown function variable: {}", id.name),
                            span.clone(),
                            DiagnosticCode::TypeMismatch,
                        ));
                    }
                    list_ty
                } else {
                    self.diagnostics.push(Diagnostic::error(
                        "filter requires a lambda expression".to_string(),
                        span.clone(),
                        DiagnosticCode::TypeMismatch,
                    ));
                    list_ty
                }
            }
            Expr::Reduce {
                list,
                init,
                lambda,
                span,
            } => {
                // Type check list, init, and lambda body
                let list_ty = self.check_expr(list);
                let init_ty = self.check_expr(init);
                let elem_ty = match &list_ty {
                    CheckedType::List(inner) => *inner.clone(),
                    CheckedType::Unknown => CheckedType::Unknown,
                    _ => {
                        self.diagnostics.push(Diagnostic::error(
                            format!("reduce requires a list, got {:?}", list_ty),
                            span.clone(),
                            DiagnosticCode::TypeMismatch,
                        ));
                        CheckedType::Unknown
                    }
                };

                if let Expr::Lambda { params, body, .. } = lambda.as_ref() {
                    let prev_locals = self.locals.clone();
                    // Bind acc and elem params
                    if params.len() >= 2 {
                        self.locals.insert(params[0].name.clone(), init_ty.clone());
                        self.locals.insert(params[1].name.clone(), elem_ty);
                    } else if !params.is_empty() {
                        self.locals.insert(params[0].name.clone(), init_ty.clone());
                    }
                    let _body_ty = self.check_expr(body);
                    self.locals = prev_locals;
                    // Returns same type as init
                    init_ty
                } else if let Expr::Ident(id) = lambda.as_ref() {
                    // Function variable - accept it if it exists in scope
                    if self.locals.get(&id.name).is_none() {
                        self.diagnostics.push(Diagnostic::error(
                            format!("unknown function variable: {}", id.name),
                            span.clone(),
                            DiagnosticCode::TypeMismatch,
                        ));
                    }
                    init_ty
                } else {
                    self.diagnostics.push(Diagnostic::error(
                        "reduce requires a lambda expression with 2 params".to_string(),
                        span.clone(),
                        DiagnosticCode::TypeMismatch,
                    ));
                    init_ty
                }
            }
            Expr::RecordLiteral {
                type_name,
                fields,
                span,
                ..
            } => {
                // Type check all field values
                for (_, expr) in fields {
                    self.check_expr(expr);
                }

                // If type_name is provided, resolve and validate
                if let Some(type_id) = type_name {
                    let name = type_id.name.clone();
                    // Clone the type info to avoid borrowing issues
                    let type_kind = self.resolve_type_info(&name).map(|i| i.kind.clone());

                    match type_kind {
                        Some(TypeKind::Record {
                            fields: expected_fields,
                        }) => {
                            // Validate field names
                            for (field_id, _) in fields {
                                if !expected_fields.contains_key(&field_id.name) {
                                    self.diagnostics.push(Diagnostic::error(
                                        format!(
                                            "Unknown field '{}' in record type '{}'",
                                            field_id.name, name
                                        ),
                                        span.clone(),
                                        DiagnosticCode::TypeMismatch,
                                    ));
                                }
                            }
                            CheckedType::Named(name)
                        }
                        Some(_) => {
                            self.diagnostics.push(Diagnostic::error(
                                format!("Type '{}' is not a record", name),
                                span.clone(),
                                DiagnosticCode::TypeMismatch,
                            ));
                            CheckedType::Named(name)
                        }
                        None => {
                            self.diagnostics.push(Diagnostic::error(
                                format!("Unknown type: {}", name),
                                span.clone(),
                                DiagnosticCode::UnknownType,
                            ));
                            CheckedType::Unknown
                        }
                    }
                } else {
                    // Anonymous record - return Unknown for now
                    CheckedType::Unknown
                }
            }
            Expr::VariantLiteral {
                type_name,
                case_name,
                payload,
                span,
            } => {
                // Type check the payload if present
                if let Some(p) = payload {
                    self.check_expr(p);
                }

                // If type_name is provided, resolve and validate
                if let Some(type_id) = type_name {
                    let name = type_id.name.clone();
                    // Clone the type info to avoid borrowing issues
                    let type_kind = self.resolve_type_info(&name).map(|i| i.kind.clone());

                    match type_kind {
                        Some(TypeKind::Variant { cases }) => {
                            // Validate case name exists and payload arity matches
                            match cases.get(&case_name.name) {
                                None => {
                                    self.diagnostics.push(Diagnostic::error(
                                        format!(
                                            "Unknown case '{}' in variant type '{}'",
                                            case_name.name, name
                                        ),
                                        span.clone(),
                                        DiagnosticCode::TypeMismatch,
                                    ));
                                }
                                Some(expects_payload) => {
                                    if *expects_payload && payload.is_none() {
                                        self.diagnostics.push(Diagnostic::error(
                                            format!(
                                                "Case '{}#{}' requires a payload",
                                                name, case_name.name
                                            ),
                                            span.clone(),
                                            DiagnosticCode::TypeMismatch,
                                        ));
                                    } else if !*expects_payload && payload.is_some() {
                                        self.diagnostics.push(Diagnostic::error(
                                            format!(
                                                "Case '{}#{}' does not accept a payload",
                                                name, case_name.name
                                            ),
                                            span.clone(),
                                            DiagnosticCode::TypeMismatch,
                                        ));
                                    }
                                }
                            }
                            if name == "result" {
                                if let Some(ref ret) = self.current_return_type {
                                    if let CheckedType::Result { .. } = ret {
                                        ret.clone()
                                    } else {
                                        CheckedType::Named(name)
                                    }
                                } else {
                                    CheckedType::Named(name)
                                }
                            } else {
                                CheckedType::Named(name)
                            }
                        }
                        Some(_) => {
                            self.diagnostics.push(Diagnostic::error(
                                format!("Type '{}' is not a variant", name),
                                span.clone(),
                                DiagnosticCode::TypeMismatch,
                            ));
                            CheckedType::Named(name)
                        }
                        None => {
                            self.diagnostics.push(Diagnostic::error(
                                format!("Unknown type: {}", name),
                                span.clone(),
                                DiagnosticCode::UnknownType,
                            ));
                            CheckedType::Unknown
                        }
                    }
                } else {
                    // Unqualified variant like #ok(42) - Unknown until context is known
                    CheckedType::Unknown
                }
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                // Type check the scrutinee
                self.check_expr(scrutinee);
                let mut inferred_arm_type: Option<CheckedType> = None;
                // Type check each arm's body expression
                for arm in arms {
                    match &arm.pattern {
                        Pattern::Variant {
                            type_name,
                            case_name,
                            binding,
                            span,
                        } => {
                            if let Some(type_id) = type_name {
                                let variant_name = type_id.name.clone();
                                let type_kind = self
                                    .resolve_type_info(&variant_name)
                                    .map(|i| i.kind.clone());

                                match type_kind {
                                    Some(TypeKind::Variant { cases }) => {
                                        match cases.get(&case_name.name) {
                                            None => {
                                                self.diagnostics.push(Diagnostic::error(
                                                    format!(
                                                        "Unknown case '{}' in variant type '{}'",
                                                        case_name.name, variant_name
                                                    ),
                                                    span.clone(),
                                                    DiagnosticCode::TypeMismatch,
                                                ));
                                            }
                                            Some(expects_payload) => {
                                                if *expects_payload && binding.is_none() {
                                                    self.diagnostics.push(Diagnostic::error(
                                                    format!(
                                                        "Case '{}#{}' pattern requires a binding for payload",
                                                        variant_name, case_name.name
                                                    ),
                                                    span.clone(),
                                                    DiagnosticCode::TypeMismatch,
                                                ));
                                                } else if !*expects_payload && binding.is_some() {
                                                    self.diagnostics.push(Diagnostic::error(
                                                    format!(
                                                        "Case '{}#{}' pattern must not bind a payload",
                                                        variant_name, case_name.name
                                                    ),
                                                    span.clone(),
                                                    DiagnosticCode::TypeMismatch,
                                                ));
                                                }
                                            }
                                        }
                                    }
                                    Some(_) => {
                                        self.diagnostics.push(Diagnostic::error(
                                            format!("Type '{}' is not a variant", variant_name),
                                            span.clone(),
                                            DiagnosticCode::TypeMismatch,
                                        ));
                                    }
                                    None => {
                                        self.diagnostics.push(Diagnostic::error(
                                            format!("Unknown type: {}", variant_name),
                                            span.clone(),
                                            DiagnosticCode::UnknownType,
                                        ));
                                    }
                                }
                            }

                            if let Some(id) = binding {
                                self.locals.insert(id.name.clone(), CheckedType::Unknown);
                            }
                        }
                        _ => {}
                    };

                    let saved_locals = self.locals.clone();
                    let arm_result = self.check_block_tail_type(&arm.body);
                    self.locals = saved_locals;

                    if arm_result != CheckedType::Unknown {
                        match &inferred_arm_type {
                            Some(existing) if *existing != arm_result => {
                                inferred_arm_type = Some(CheckedType::Unknown);
                            }
                            None => inferred_arm_type = Some(arm_result),
                            _ => {}
                        }
                    }
                }
                // Match expressions return the type of the arms
                inferred_arm_type.unwrap_or(CheckedType::Unknown)
            }
            Expr::While {
                condition, body, ..
            } => {
                // Type check the condition (should be bool)
                let cond_ty = self.check_expr(condition);
                if cond_ty != CheckedType::Bool && cond_ty != CheckedType::Unknown {
                    self.diagnostics.push(Diagnostic::error(
                        format!("While condition must be bool, got {:?}", cond_ty),
                        Span::default(),
                        DiagnosticCode::TypeMismatch,
                    ));
                }
                let saved_locals = self.locals.clone();
                self.loop_depth += 1;
                self.validate_block(body);
                self.loop_depth -= 1;
                self.locals = saved_locals;
                // While loops don't produce a value (unit type)
                CheckedType::Unknown
            }
            Expr::Range { start, end, .. } => {
                // Both start and end must be integers
                let start_ty = self.check_expr(start);
                let end_ty = self.check_expr(end);
                if !self.is_numeric(&start_ty) && start_ty != CheckedType::Unknown {
                    self.diagnostics.push(Diagnostic::error(
                        format!("Range start must be integer, got {:?}", start_ty),
                        Span::default(),
                        DiagnosticCode::TypeMismatch,
                    ));
                }
                if !self.is_numeric(&end_ty) && end_ty != CheckedType::Unknown {
                    self.diagnostics.push(Diagnostic::error(
                        format!("Range end must be integer, got {:?}", end_ty),
                        Span::default(),
                        DiagnosticCode::TypeMismatch,
                    ));
                }
                // Range itself is a special type (we'll treat as Unknown for now)
                CheckedType::Unknown
            }
            Expr::For {
                variable,
                range,
                body,
                ..
            } => {
                // Type check the range
                self.check_expr(range);
                let saved_locals = self.locals.clone();
                self.locals.insert(variable.name.clone(), CheckedType::I32);
                self.loop_depth += 1;
                self.validate_block(body);
                self.loop_depth -= 1;
                self.locals = saved_locals;
                // For loops don't produce a value (unit type)
                CheckedType::Unknown
            }
            Expr::ForEach {
                variable,
                collection,
                body,
                span,
            } => {
                // Type check the collection
                let collection_ty = self.check_expr(collection);
                // Determine element type from collection
                let elem_ty = match collection_ty {
                    CheckedType::List(inner) => *inner,
                    CheckedType::Unknown => CheckedType::Unknown,
                    _ => {
                        self.diagnostics.push(Diagnostic::error(
                            format!("for-each requires list, got {:?}", collection_ty),
                            span.clone(),
                            DiagnosticCode::TypeMismatch,
                        ));
                        CheckedType::Unknown
                    }
                };
                let saved_locals = self.locals.clone();
                self.locals.insert(variable.name.clone(), elem_ty);
                self.loop_depth += 1;
                self.validate_block(body);
                self.loop_depth -= 1;
                self.locals = saved_locals;
                // For-each loops don't produce a value (unit type)
                CheckedType::Unknown
            }
            Expr::ListLiteral { elements, span } => {
                // Type check all elements and infer list type from first element
                let element_type = if elements.is_empty() {
                    CheckedType::Unknown
                } else {
                    let first_ty = self.check_expr(&elements[0]);
                    // Check all elements have same type
                    for (i, elem) in elements.iter().skip(1).enumerate() {
                        let elem_ty = self.check_expr(elem);
                        if elem_ty != first_ty
                            && elem_ty != CheckedType::Unknown
                            && first_ty != CheckedType::Unknown
                        {
                            self.diagnostics.push(Diagnostic::error(
                                format!(
                                    "List element {} has type {:?}, expected {:?}",
                                    i + 1,
                                    elem_ty,
                                    first_ty
                                ),
                                span.clone(),
                                DiagnosticCode::TypeMismatch,
                            ));
                        }
                    }
                    first_ty
                };
                CheckedType::List(Box::new(element_type))
            }
            Expr::Index { expr, index, span } => {
                // Check the base expression is a list
                let base_ty = self.check_expr(expr);
                // Check the index is an integer
                let index_ty = self.check_expr(index);
                if !self.is_numeric(&index_ty) && index_ty != CheckedType::Unknown {
                    self.diagnostics.push(Diagnostic::error(
                        format!("List index must be integer, got {:?}", index_ty),
                        span.clone(),
                        DiagnosticCode::TypeMismatch,
                    ));
                }
                // Return the element type
                match base_ty {
                    CheckedType::List(elem_ty) => *elem_ty,
                    CheckedType::Unknown => CheckedType::Unknown,
                    _ => {
                        self.diagnostics.push(Diagnostic::error(
                            format!("Cannot index into non-list type {:?}", base_ty),
                            span.clone(),
                            DiagnosticCode::TypeMismatch,
                        ));
                        CheckedType::Unknown
                    }
                }
            }
            Expr::Slice {
                expr,
                start,
                end,
                span,
            } => {
                // Check the base expression is a list
                let base_ty = self.check_expr(expr);
                // Check start and end are integers
                let start_ty = self.check_expr(start);
                let end_ty = self.check_expr(end);
                if !self.is_numeric(&start_ty) && start_ty != CheckedType::Unknown {
                    self.diagnostics.push(Diagnostic::error(
                        format!("Slice start must be integer, got {:?}", start_ty),
                        span.clone(),
                        DiagnosticCode::TypeMismatch,
                    ));
                }
                if !self.is_numeric(&end_ty) && end_ty != CheckedType::Unknown {
                    self.diagnostics.push(Diagnostic::error(
                        format!("Slice end must be integer, got {:?}", end_ty),
                        span.clone(),
                        DiagnosticCode::TypeMismatch,
                    ));
                }
                // Return the same list type
                match &base_ty {
                    CheckedType::List(_) => base_ty,
                    CheckedType::Unknown => CheckedType::Unknown,
                    _ => {
                        self.diagnostics.push(Diagnostic::error(
                            format!("Cannot slice non-list type {:?}", base_ty),
                            span.clone(),
                            DiagnosticCode::TypeMismatch,
                        ));
                        CheckedType::Unknown
                    }
                }
            }
            Expr::OptionalChain { expr, field, .. } => {
                let base_ty = self.check_expr(expr);
                match base_ty {
                    CheckedType::Option(inner) => match inner.as_ref() {
                        CheckedType::Named(type_name) => {
                            let type_kind =
                                self.resolve_type_info(type_name).map(|i| i.kind.clone());
                            match type_kind {
                                Some(TypeKind::Record { fields }) => {
                                    if let Some(field_ty) = fields.get(&field.name) {
                                        CheckedType::Option(Box::new(self.ty_to_checked(field_ty)))
                                    } else {
                                        self.diagnostics.push(Diagnostic::error(
                                            format!(
                                                "Record type '{}' has no field '{}'",
                                                type_name, field.name
                                            ),
                                            field.span.clone(),
                                            DiagnosticCode::UnknownVariable,
                                        ));
                                        CheckedType::Unknown
                                    }
                                }
                                _ => CheckedType::Unknown,
                            }
                        }
                        CheckedType::Unknown => CheckedType::Unknown,
                        _ => CheckedType::Unknown,
                    },
                    CheckedType::Unknown => CheckedType::Unknown,
                    _ => {
                        self.diagnostics.push(Diagnostic::error(
                            "Optional chaining requires option value".to_string(),
                            field.span.clone(),
                            DiagnosticCode::TypeMismatch,
                        ));
                        CheckedType::Unknown
                    }
                }
            }
            Expr::Try { expr, span } => {
                let base_ty = self.check_expr(expr);
                match base_ty {
                    CheckedType::Option(inner) => *inner,
                    CheckedType::Result { ok, .. } => {
                        ok.map(|inner| *inner).unwrap_or(CheckedType::Unknown)
                    }
                    CheckedType::Unknown => CheckedType::Unknown,
                    _ => {
                        self.diagnostics.push(Diagnostic::error(
                            "Try operator requires option or result value".to_string(),
                            span.clone(),
                            DiagnosticCode::TypeMismatch,
                        ));
                        CheckedType::Unknown
                    }
                }
            }
            Expr::Await { expr, span } => {
                let base_ty = self.check_expr(expr);
                match base_ty {
                    CheckedType::Future(Some(inner)) => *inner,
                    CheckedType::Future(None) | CheckedType::Unknown => CheckedType::Unknown,
                    _ => {
                        self.diagnostics.push(Diagnostic::error(
                            "await requires future value".to_string(),
                            span.clone(),
                            DiagnosticCode::TypeMismatch,
                        ));
                        CheckedType::Unknown
                    }
                }
            }
            // Atomic operations — all return i32 (or void for store)
            Expr::AtomicLoad { addr, .. } => {
                self.check_expr(addr);
                CheckedType::I32
            }
            Expr::AtomicStore { addr, value, .. } => {
                self.check_expr(addr);
                self.check_expr(value);
                CheckedType::Unknown // void
            }
            Expr::AtomicAdd { addr, value, .. } | Expr::AtomicSub { addr, value, .. } => {
                self.check_expr(addr);
                self.check_expr(value);
                CheckedType::I32 // returns old value
            }
            Expr::AtomicCmpxchg {
                addr,
                expected,
                replacement,
                ..
            } => {
                self.check_expr(addr);
                self.check_expr(expected);
                self.check_expr(replacement);
                CheckedType::I32 // returns old value
            }
            Expr::AtomicWait {
                addr,
                expected,
                timeout,
                ..
            } => {
                self.check_expr(addr);
                self.check_expr(expected);
                self.check_expr(timeout);
                CheckedType::I32 // 0=ok, 1=mismatch, 2=timeout
            }
            Expr::AtomicNotify { addr, count, .. } => {
                self.check_expr(addr);
                self.check_expr(count);
                CheckedType::I32 // returns num waiters woken
            }
            Expr::Spawn { body, .. } => {
                for s in body {
                    self.validate_statement(s);
                }
                CheckedType::ThreadId
            }
            Expr::ThreadJoin { tid, span } => {
                let tid_ty = self.check_expr(tid);
                if tid_ty != CheckedType::ThreadId {
                    self.diagnostics.push(Diagnostic::error(
                        format!("thread.join requires a thread-id, got {:?}", tid_ty),
                        span.clone(),
                        DiagnosticCode::TypeMismatch,
                    ));
                }
                CheckedType::I32
            }
            Expr::AtomicBlock { body, .. } => {
                for s in body {
                    self.validate_statement(s);
                }
                // Atomic block evaluates to the type of its last expression (typically I32)
                CheckedType::I32
            }
            Expr::SimdOp { args, op, .. } => {
                // Check all args
                for arg in args {
                    self.check_expr(arg);
                }
                // extract_lane returns a scalar; everything else returns v128
                match op {
                    SimdOp::ExtractLane | SimdOp::AnyTrue | SimdOp::AllTrue | SimdOp::Bitmask => {
                        CheckedType::I32
                    }
                    _ => CheckedType::V128,
                }
            }
            Expr::SimdForEach {
                variable,
                collection,
                body,
                ..
            } => {
                self.check_expr(collection);
                let saved_locals = self.locals.clone();
                self.locals.insert(variable.name.clone(), CheckedType::V128);
                self.loop_depth += 1;
                self.validate_block(body);
                self.loop_depth -= 1;
                self.locals = saved_locals;
                CheckedType::Unknown
            }
        }
    }

    fn is_numeric(&self, ty: &CheckedType) -> bool {
        matches!(
            ty,
            CheckedType::I32
                | CheckedType::I64
                | CheckedType::F32
                | CheckedType::F64
                | CheckedType::Unknown
        )
    }

    fn ty_to_checked(&self, ty: &Ty) -> CheckedType {
        match ty {
            Ty::Primitive(p, _) => match p {
                PrimitiveTy::Bool => CheckedType::Bool,
                PrimitiveTy::S32 | PrimitiveTy::U32 => CheckedType::I32,
                PrimitiveTy::S64 | PrimitiveTy::U64 => CheckedType::I64,
                PrimitiveTy::F32 => CheckedType::F32,
                PrimitiveTy::F64 => CheckedType::F64,
                PrimitiveTy::String | PrimitiveTy::Char => CheckedType::String,
                _ => CheckedType::Unknown,
            },
            Ty::Named(id) => CheckedType::Named(id.name.clone()),
            Ty::List { element, .. } => CheckedType::List(Box::new(self.ty_to_checked(element))),
            Ty::Option { inner, .. } => CheckedType::Option(Box::new(self.ty_to_checked(inner))),
            Ty::Result { ok, err, .. } => CheckedType::Result {
                ok: ok.as_ref().map(|t| Box::new(self.ty_to_checked(t))),
                err: err.as_ref().map(|t| Box::new(self.ty_to_checked(t))),
            },
            Ty::Future { inner, .. } => {
                CheckedType::Future(inner.as_ref().map(|t| Box::new(self.ty_to_checked(t))))
            }
            _ => CheckedType::Unknown,
        }
    }

    fn fmt_ty(&self, ty: &Ty) -> String {
        match ty {
            Ty::Primitive(p, _) => format!("{:?}", p).to_lowercase(),
            Ty::Named(id) => id.name.clone(),
            Ty::List { element, .. } => format!("list<{}>", self.fmt_ty(element)),
            Ty::Option { inner, .. } => format!("option<{}>", self.fmt_ty(inner)),
            Ty::Result { ok, err, .. } => {
                let ok_str = ok
                    .as_ref()
                    .map(|t| self.fmt_ty(t))
                    .unwrap_or_else(|| "()".to_string());
                let err_str = err
                    .as_ref()
                    .map(|t| self.fmt_ty(t))
                    .unwrap_or_else(|| "()".to_string());
                format!("result<{}, {}>", ok_str, err_str)
            }
            Ty::Future { inner, .. } => {
                if let Some(t) = inner {
                    format!("future<{}>", self.fmt_ty(t))
                } else {
                    "future".to_string()
                }
            }
            Ty::Stream { inner, .. } => {
                if let Some(t) = inner {
                    format!("stream<{}>", self.fmt_ty(t))
                } else {
                    "stream".to_string()
                }
            }
            Ty::Tuple { elements, .. } => {
                let inner = elements
                    .iter()
                    .map(|t| self.fmt_ty(t))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({})", inner)
            }
            Ty::Borrow { resource, .. } => format!("borrow<{}>", resource.name),
            Ty::Own { resource, .. } => format!("own<{}>", resource.name),
            Ty::Generic { name, args, .. } => {
                let inner = args
                    .iter()
                    .map(|t| self.fmt_ty(t))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}<{}>", name.name, inner)
            }
        }
    }

    fn validate_type(&mut self, ty: &Ty) {
        match ty {
            Ty::Named(id) => {
                if !self.resolve_type(&id.name) {
                    self.diagnostics.push(Diagnostic::error(
                        format!("Unknown type: {}", id.name),
                        id.span.clone(),
                        DiagnosticCode::UnknownType,
                    ));
                }
            }
            Ty::List { element, .. } => self.validate_type(element),
            Ty::Option { inner, .. } => self.validate_type(inner),
            Ty::Result { ok, err, .. } => {
                if let Some(ok) = ok {
                    self.validate_type(ok);
                }
                if let Some(err) = err {
                    self.validate_type(err);
                }
            }
            Ty::Tuple { elements, .. } => {
                for elem in elements {
                    self.validate_type(elem);
                }
            }
            Ty::Future { inner, .. } | Ty::Stream { inner, .. } => {
                if let Some(inner) = inner {
                    self.validate_type(inner);
                }
            }
            Ty::Borrow { resource, .. } | Ty::Own { resource, .. } => {
                if !self.resolve_type(&resource.name) {
                    self.diagnostics.push(Diagnostic::error(
                        format!("Unknown resource: {}", resource.name),
                        resource.span.clone(),
                        DiagnosticCode::UnknownResource,
                    ));
                }
            }
            Ty::Generic { name, args, .. } => {
                // Validate the generic type name exists (e.g., pair<s32>)
                if !self.resolve_type(&name.name) {
                    self.diagnostics.push(Diagnostic::error(
                        format!("Unknown type: {}", name.name),
                        name.span.clone(),
                        DiagnosticCode::UnknownType,
                    ));
                }
                // Recursively validate type arguments
                for arg in args {
                    self.validate_type(arg);
                }
            }
            Ty::Primitive(_, _) => {}
        }
    }

    /// Resolve a type name in the current scope
    fn resolve_type(&self, name: &str) -> bool {
        if self.type_params.contains(name) {
            return true;
        }
        if [
            "bool", "s8", "s16", "s32", "s64", "u8", "u16", "u32", "u64", "f32", "f64", "char",
            "string", "list", "option", "result",
        ]
        .contains(&name)
        {
            return true;
        }
        self.resolve_type_info(name).is_some()
    }

    /// Resolve a type name and return its info
    fn resolve_type_info(&self, name: &str) -> Option<&TypeInfo> {
        // First try interface-local scope
        if let Some(ref iface) = self.current_interface {
            let key = TypeKey {
                interface: Some(iface.clone()),
                name: name.to_string(),
            };
            if let Some(info) = self.types.get(&key) {
                return Some(info);
            }
        }

        // Then try global scope
        let key = TypeKey {
            interface: None,
            name: name.to_string(),
        };
        self.types.get(&key)
    }

    fn resolve_type_alias_constraint(&self, ty: &Ty) -> Option<Expr> {
        match ty {
            Ty::Named(id) => {
                let info = self.resolve_type_info(&id.name)?;
                match &info.kind {
                    TypeKind::Alias { constraint, .. } => constraint.clone(),
                    _ => None,
                }
            }
            Ty::Generic { name, .. } => {
                let info = self.resolve_type_info(&name.name)?;
                match &info.kind {
                    TypeKind::Alias { constraint, .. } => constraint.clone(),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn validate_import_export(&mut self, ie: &ImportExport) {
        match &ie.kind {
            ImportExportKind::Path(path) => {
                // Validate the interface exists
                if path.package.is_none() && !self.interfaces.contains_key(&path.interface.name) {
                    self.diagnostics.push(Diagnostic::error(
                        format!("Unknown interface: {}", path.interface.name),
                        ie.span.clone(),
                        DiagnosticCode::UnknownInterface,
                    ));
                }
            }
            ImportExportKind::Func(func) => {
                self.validate_func(func);
            }
            ImportExportKind::Interface(items) => {
                for item in items {
                    match item {
                        InterfaceItem::TypeDef(typedef) => self.validate_typedef(typedef),
                        InterfaceItem::Func(func) => self.validate_func(func),
                        InterfaceItem::Use(use_stmt) => self.validate_use_statement(use_stmt),
                    }
                }
            }
        }
    }

    fn validate_use_statement(&mut self, use_stmt: &UseStatement) {
        // Validate the source interface exists
        if use_stmt.path.package.is_none()
            && !self.interfaces.contains_key(&use_stmt.path.interface.name)
        {
            self.diagnostics.push(Diagnostic::error(
                format!(
                    "Unknown interface in use statement: {}",
                    use_stmt.path.interface.name
                ),
                use_stmt.span.clone(),
                DiagnosticCode::InvalidUse,
            ));
            return;
        }

        // Validate each imported name exists in the source interface
        if let Some(iface_info) = self.interfaces.get(&use_stmt.path.interface.name) {
            for item in &use_stmt.names {
                if !iface_info.types.contains(&item.name.name)
                    && !iface_info.functions.contains(&item.name.name)
                {
                    self.diagnostics.push(Diagnostic::error(
                        format!(
                            "Interface '{}' does not export '{}'",
                            use_stmt.path.interface.name, item.name.name
                        ),
                        item.name.span.clone(),
                        DiagnosticCode::InvalidUse,
                    ));
                }
            }
        }
    }

    fn validate_top_level_use(&mut self, use_stmt: &TopLevelUse) {
        // Validate the source interface exists (for local uses)
        if use_stmt.path.package.is_none()
            && !self.interfaces.contains_key(&use_stmt.path.interface.name)
        {
            self.diagnostics.push(Diagnostic::error(
                format!("Unknown interface: {}", use_stmt.path.interface.name),
                use_stmt.span.clone(),
                DiagnosticCode::InvalidUse,
            ));
        }
    }

    fn validate_include(&mut self, include: &IncludeStatement) {
        // Validate the included world/interface exists
        if include.path.package.is_none() {
            let name = &include.path.interface.name;
            if !self.worlds.contains_key(name) && !self.interfaces.contains_key(name) {
                self.diagnostics.push(Diagnostic::error(
                    format!("Unknown world or interface to include: {}", name),
                    include.span.clone(),
                    DiagnosticCode::InvalidUse,
                ));
            }
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

fn typedef_name(kind: &TypeDefKind) -> String {
    match kind {
        TypeDefKind::Alias { name, .. } => name.name.clone(),
        TypeDefKind::Record { name, .. } => name.name.clone(),
        TypeDefKind::Variant { name, .. } => name.name.clone(),
        TypeDefKind::Enum { name, .. } => name.name.clone(),
        TypeDefKind::Flags { name, .. } => name.name.clone(),
        TypeDefKind::Resource { name, .. } => name.name.clone(),
    }
}

fn typedef_to_kind(kind: &TypeDefKind) -> TypeKind {
    match kind {
        TypeDefKind::Alias { ty, constraint, .. } => TypeKind::Alias {
            target: Box::new(ty.clone()),
            constraint: constraint.clone(),
        },
        TypeDefKind::Record { fields, .. } => TypeKind::Record {
            fields: fields
                .iter()
                .map(|f| (f.name.name.clone(), f.ty.clone()))
                .collect(),
        },
        TypeDefKind::Variant { cases, .. } => TypeKind::Variant {
            cases: cases
                .iter()
                .map(|c| (c.name.name.clone(), c.ty.is_some()))
                .collect(),
        },
        TypeDefKind::Enum { cases, .. } => TypeKind::Enum {
            cases: cases.iter().map(|c| c.name.clone()).collect(),
        },
        TypeDefKind::Flags { flags, .. } => TypeKind::Flags {
            flags: flags.iter().map(|f| f.name.clone()).collect(),
        },
        TypeDefKind::Resource { .. } => TypeKind::Resource,
    }
}

fn substitute_expr_ident(expr: &Expr, from: &str, to: &str) -> Expr {
    match expr {
        Expr::Ident(id) if id.name == from => Expr::Ident(Id {
            name: to.to_string(),
            span: id.span.clone(),
        }),
        Expr::Binary { lhs, op, rhs, span } => Expr::Binary {
            lhs: Box::new(substitute_expr_ident(lhs, from, to)),
            op: *op,
            rhs: Box::new(substitute_expr_ident(rhs, from, to)),
            span: span.clone(),
        },
        Expr::Not(inner, span) => Expr::Not(
            Box::new(substitute_expr_ident(inner, from, to)),
            span.clone(),
        ),
        Expr::Neg(inner, span) => Expr::Neg(
            Box::new(substitute_expr_ident(inner, from, to)),
            span.clone(),
        ),
        other => other.clone(),
    }
}
fn package_path_to_string(path: &PackagePath) -> String {
    let namespace = path
        .namespace
        .iter()
        .map(|id| id.name.as_str())
        .collect::<Vec<_>>();
    let name = path
        .name
        .iter()
        .map(|id| id.name.as_str())
        .collect::<Vec<_>>();

    let namespace_len: usize =
        namespace.iter().map(|s| s.len()).sum::<usize>() + namespace.len().saturating_sub(1);
    let name_len: usize =
        name.iter().map(|s| s.len()).sum::<usize>() + name.len().saturating_sub(1);

    // Reserve extra space for separators and optional version info.
    let mut result = String::with_capacity(namespace_len + name_len + 16);

    if !namespace.is_empty() {
        result.push_str(&namespace.join(":"));
        result.push(':');
    }

    result.push_str(&name.join("/"));

    if let Some(version) = &path.version {
        result.push('@');
        result.push_str(&format!(
            "{}.{}.{}",
            version.major, version.minor, version.patch
        ));
        if let Some(prerelease) = &version.prerelease {
            result.push('-');
            result.push_str(prerelease);
        }
    }

    result
}

// ============================================================================
// Constraint Checking Methods
// ============================================================================

impl Checker {
    fn check_call_constraints(&mut self, func: &Id, args: &[Expr], span: &Span) {
        if self.in_guard_let {
            return;
        }
        if let Some(iface_name) = &self.current_interface {
            if let Some(iface) = self.interfaces.get(iface_name) {
                let mut param_to_arg: HashMap<String, &Expr> = HashMap::new();
                if let Some(param_names) = iface.function_params.get(&func.name) {
                    for (i, param_name) in param_names.iter().enumerate() {
                        if i < args.len() {
                            param_to_arg.insert(param_name.clone(), &args[i]);
                        }
                    }
                }

                // Direct constraints
                if let Some(constraints) = iface.function_constraints.get(&func.name) {
                    for constraint in constraints {
                        let param_index =
                            iface.function_params.get(&func.name).and_then(|params| {
                                params.iter().position(|p| p == &constraint.param_name)
                            });

                        if let Some(idx) = param_index {
                            if idx < args.len() {
                                let arg_expr = &args[idx];
                                let arg_value = self.const_value(arg_expr);
                                let eval_result = self.eval_constraint_with_mapping(
                                    &constraint.constraint,
                                    &constraint.param_name,
                                    arg_value,
                                    &param_to_arg,
                                );
                                let is_test = self.is_in_test_function();
                                let error_msg = match &eval_result {
                                    ConstraintEvalResult::Violated(_) => {
                                        let msg = self.fmt_constraint_with_values(
                                            &constraint.constraint,
                                            &constraint.param_name,
                                            arg_value.unwrap_or(0),
                                            &param_to_arg,
                                        );
                                        let arg_name = self.get_arg_name(arg_expr);
                                        if arg_name != constraint.param_name {
                                            format!(
                                                "{} does not satisfy the constraint \"{}\" on {}",
                                                arg_name, msg, func.name
                                            )
                                        } else {
                                            format!(
                                                "{} does not satisfy the constraint \"{}\" on {}",
                                                constraint.param_name, msg, func.name
                                            )
                                        }
                                    }
                                    ConstraintEvalResult::NeedsPropagation(vars) => {
                                        let var_names_plain: Vec<String> = vars
                                            .iter()
                                            .map(|v| {
                                                param_to_arg
                                                    .get(v)
                                                    .map(|e| self.get_arg_name(e))
                                                    .unwrap_or_else(|| v.clone())
                                            })
                                            .collect();
                                        if vars.iter().all(|v| param_to_arg.contains_key(v)) {
                                            let arg_name = param_to_arg
                                                .get(&constraint.param_name)
                                                .map(|e| self.get_arg_name(e))
                                                .unwrap_or_else(|| constraint.param_name.clone());
                                            let constraint_msg = self.fmt_constraint_with_values(
                                                &constraint.constraint,
                                                &constraint.param_name,
                                                arg_value.unwrap_or(0),
                                                &param_to_arg,
                                            );
                                            format!(
                                                "{} may not satisfy the constraint \"{}\" because {} is an unconstrained parameter, {} must be called with a guard",
                                                arg_name,
                                                constraint_msg,
                                                var_names_plain.join(" and "),
                                                func.name
                                            )
                                        } else {
                                            format!(
                                                "Cannot verify constraint on '{}': unresolved variables {:?}",
                                                constraint.param_name, vars
                                            )
                                        }
                                    }
                                    _ => String::new(),
                                };

                                if is_test && !error_msg.is_empty() {
                                    let is_hushed = self.check_hush_comment(span, &error_msg);
                                    if is_hushed {
                                        self.diagnostics.push(Diagnostic::info(
                                            error_msg,
                                            span.clone(),
                                            DiagnosticCode::ConstraintViolation,
                                        ));
                                    } else {
                                        self.diagnostics.push(Diagnostic::error(
                                            error_msg,
                                            span.clone(),
                                            DiagnosticCode::ConstraintViolation,
                                        ));
                                    }
                                } else {
                                    match eval_result {
                                        ConstraintEvalResult::Violated(_) => {
                                            self.diagnostics.push(Diagnostic::error(
                                                error_msg,
                                                span.clone(),
                                                DiagnosticCode::ConstraintViolation,
                                            ));
                                        }
                                        ConstraintEvalResult::NeedsPropagation(_vars) => {
                                            self.diagnostics.push(Diagnostic::warning(
                                                error_msg,
                                                span.clone(),
                                                DiagnosticCode::ConstraintPropagation,
                                            ));
                                        }
                                        ConstraintEvalResult::Satisfied => {}
                                    }
                                }
                            }
                        }
                    }
                }

                // Inherited (transitive) constraints
                let inherited: Vec<InheritedConstraint> = iface
                    .inherited_constraints
                    .get(&func.name)
                    .cloned()
                    .unwrap_or_default();
                for inh in &inherited {
                    self.check_inherited_constraint(inh, &param_to_arg, span);
                }
            }
        }
    }

    fn check_inherited_constraint(
        &mut self,
        inh: &InheritedConstraint,
        param_to_arg: &HashMap<String, &Expr>,
        span: &Span,
    ) {
        let mut target_values: HashMap<String, Option<i64>> = HashMap::new();
        let mut target_param_to_caller_arg: HashMap<String, &Expr> = HashMap::new();
        let mut all_resolved = true;
        let mut unresolved_vars: Vec<String> = Vec::new();

        for (target_param, source) in &inh.target_param_sources {
            match source {
                ParamSource::Constant(val) => {
                    target_values.insert(target_param.clone(), Some(*val));
                }
                ParamSource::Param(intermediate_param) => {
                    let lookup_key: &str = intermediate_param.as_str();
                    if let Some(caller_arg) = param_to_arg.get(lookup_key) {
                        let val = self.const_value(*caller_arg);
                        target_values.insert(target_param.clone(), val);
                        target_param_to_caller_arg.insert(target_param.clone(), *caller_arg);
                    } else {
                        all_resolved = false;
                        unresolved_vars.push(intermediate_param.clone());
                    }
                }
            }
        }

        let constraint = &inh.constraint;
        let arg_value = target_values.get(&constraint.param_name).copied().flatten();

        let eval_result = self.eval_constraint_with_value_map(
            &constraint.constraint,
            &constraint.param_name,
            arg_value,
            &target_values,
        );

        let via_suffix = format!(" (via {})", inh.via.join(" via "));
        let is_test = self.is_in_test_function();

        let error_msg = match &eval_result {
            ConstraintEvalResult::Violated(_) => {
                let msg = self.fmt_constraint_with_value_map(
                    &constraint.constraint,
                    &constraint.param_name,
                    arg_value.unwrap_or(0),
                    &target_values,
                );
                let arg_name =
                    if let Some(e) = target_param_to_caller_arg.get(&constraint.param_name) {
                        self.get_arg_name(*e)
                    } else {
                        let mut sorted_keys: Vec<_> = target_param_to_caller_arg.keys().collect();
                        sorted_keys.sort();
                        sorted_keys
                            .into_iter()
                            .filter_map(|k| {
                                target_param_to_caller_arg
                                    .get(k)
                                    .map(|e| self.get_arg_name(*e))
                            })
                            .next()
                            .unwrap_or_else(|| constraint.param_name.clone())
                    };
                format!(
                    "{} does not satisfy the constraint \"{}\" on {}{}",
                    arg_name, msg, inh.target_func, via_suffix
                )
            }
            ConstraintEvalResult::NeedsPropagation(vars) => {
                if all_resolved && vars.is_empty() {
                    return;
                }
                let var_names_plain: Vec<String> = vars
                    .iter()
                    .map(|v| {
                        target_param_to_caller_arg
                            .get(v)
                            .map(|e| self.get_arg_name(*e))
                            .unwrap_or_else(|| v.clone())
                    })
                    .collect();
                if !all_resolved {
                    format!(
                        "Cannot verify transitive constraint on '{}': unresolved variables {:?}",
                        constraint.param_name, unresolved_vars
                    )
                } else if vars
                    .iter()
                    .all(|v| target_param_to_caller_arg.contains_key(v))
                {
                    let arg_name = target_param_to_caller_arg
                        .get(&constraint.param_name)
                        .map(|e| self.get_arg_name(*e))
                        .unwrap_or_else(|| constraint.param_name.clone());
                    let constraint_msg = self.fmt_constraint_with_value_map(
                        &constraint.constraint,
                        &constraint.param_name,
                        arg_value.unwrap_or(0),
                        &target_values,
                    );
                    format!(
                        "{} may not satisfy the constraint \"{}\" because {} is an unconstrained parameter, {} must be called with a guard{}",
                        arg_name,
                        constraint_msg,
                        var_names_plain.join(" and "),
                        inh.target_func,
                        via_suffix
                    )
                } else {
                    return;
                }
            }
            ConstraintEvalResult::Satisfied => return,
        };

        if is_test && !error_msg.is_empty() {
            let is_hushed = self.check_hush_comment(span, &error_msg);
            if is_hushed {
                self.diagnostics.push(Diagnostic::info(
                    error_msg,
                    span.clone(),
                    DiagnosticCode::ConstraintViolation,
                ));
            } else {
                self.diagnostics.push(Diagnostic::error(
                    error_msg,
                    span.clone(),
                    DiagnosticCode::ConstraintViolation,
                ));
            }
        } else {
            match eval_result {
                ConstraintEvalResult::Violated(_) => {
                    self.diagnostics.push(Diagnostic::error(
                        error_msg,
                        span.clone(),
                        DiagnosticCode::ConstraintViolation,
                    ));
                }
                ConstraintEvalResult::NeedsPropagation(_) => {
                    self.diagnostics.push(Diagnostic::warning(
                        error_msg,
                        span.clone(),
                        DiagnosticCode::ConstraintPropagation,
                    ));
                }
                ConstraintEvalResult::Satisfied => {}
            }
        }
    }

    fn get_arg_name(&self, expr: &Expr) -> String {
        match expr {
            Expr::Ident(id) => id.name.clone(),
            _ => "value".to_string(),
        }
    }

    fn eval_binary_bool(&self, op: kettu_parser::BinOp, l: i64, r: i64) -> bool {
        match op {
            kettu_parser::BinOp::Eq => l == r,
            kettu_parser::BinOp::Ne => l != r,
            kettu_parser::BinOp::Lt => l < r,
            kettu_parser::BinOp::Le => l <= r,
            kettu_parser::BinOp::Gt => l > r,
            kettu_parser::BinOp::Ge => l >= r,
            _ => false,
        }
    }

    fn eval_binary_constraint(&self, op: kettu_parser::BinOp, l: i64, r: i64) -> Option<i64> {
        match op {
            kettu_parser::BinOp::Add => Some(l + r),
            kettu_parser::BinOp::Sub => Some(l - r),
            kettu_parser::BinOp::Mul => Some(l * r),
            kettu_parser::BinOp::Div => {
                if r == 0 {
                    None
                } else {
                    Some(l / r)
                }
            }
            kettu_parser::BinOp::Eq => Some(if l == r { 1 } else { 0 }),
            kettu_parser::BinOp::Ne => Some(if l != r { 1 } else { 0 }),
            kettu_parser::BinOp::Lt => Some(if l < r { 1 } else { 0 }),
            kettu_parser::BinOp::Le => Some(if l <= r { 1 } else { 0 }),
            kettu_parser::BinOp::Gt => Some(if l > r { 1 } else { 0 }),
            kettu_parser::BinOp::Ge => Some(if l >= r { 1 } else { 0 }),
            _ => None,
        }
    }

    fn const_value(&self, expr: &Expr) -> Option<i64> {
        match expr {
            Expr::Integer(n, _) => Some(*n),
            Expr::Ident(id) => self.constants.get(&id.name).copied(),
            Expr::Neg(inner, _) => self.const_value(inner).map(|v| -v),
            Expr::Binary { lhs, op, rhs, .. } => {
                let lhs_val = self.const_value(lhs)?;
                let rhs_val = self.const_value(rhs)?;
                self.eval_binary_constraint(*op, lhs_val, rhs_val)
            }
            _ => None,
        }
    }

    fn eval_constraint_with_mapping(
        &self,
        constraint: &Expr,
        param_name: &str,
        arg_value: Option<i64>,
        param_to_arg: &HashMap<String, &Expr>,
    ) -> ConstraintEvalResult {
        match constraint {
            Expr::Binary { lhs, op, rhs, .. } => {
                let lhs_vars = self.collect_free_vars(lhs);
                let rhs_vars = self.collect_free_vars(rhs);
                let all_vars: Vec<String> =
                    lhs_vars.iter().chain(rhs_vars.iter()).cloned().collect();

                let lhs_val =
                    self.resolve_constraint_value(lhs, param_name, arg_value, param_to_arg);
                let rhs_val =
                    self.resolve_constraint_value(rhs, param_name, arg_value, param_to_arg);

                match (lhs_val, rhs_val) {
                    (Some(l), Some(r)) => {
                        if self.eval_binary_bool(*op, l, r) {
                            ConstraintEvalResult::Satisfied
                        } else {
                            ConstraintEvalResult::Violated(format!(
                                "{} {} {} is false",
                                l,
                                self.fmt_op(*op),
                                r
                            ))
                        }
                    }
                    _ if !all_vars.is_empty() => ConstraintEvalResult::NeedsPropagation(all_vars),
                    _ => ConstraintEvalResult::NeedsPropagation(vec![]),
                }
            }
            _ => ConstraintEvalResult::NeedsPropagation(vec![]),
        }
    }

    fn resolve_constraint_value(
        &self,
        expr: &Expr,
        param_name: &str,
        arg_value: Option<i64>,
        param_to_arg: &HashMap<String, &Expr>,
    ) -> Option<i64> {
        match expr {
            Expr::Integer(n, _) => Some(*n),
            Expr::Ident(id) => {
                if id.name == param_name {
                    arg_value
                } else if let Some(arg_expr) = param_to_arg.get(&id.name) {
                    self.const_value(arg_expr)
                } else {
                    self.constants.get(&id.name).copied()
                }
            }
            Expr::Binary { lhs, op, rhs, .. } => {
                let lhs_val =
                    self.resolve_constraint_value(lhs, param_name, arg_value, param_to_arg)?;
                let rhs_val =
                    self.resolve_constraint_value(rhs, param_name, arg_value, param_to_arg)?;
                self.eval_binary_constraint(*op, lhs_val, rhs_val)
            }
            _ => None,
        }
    }

    fn eval_constraint_with_value_map(
        &self,
        constraint: &Expr,
        param_name: &str,
        arg_value: Option<i64>,
        value_map: &HashMap<String, Option<i64>>,
    ) -> ConstraintEvalResult {
        match constraint {
            Expr::Binary { lhs, op, rhs, .. } => {
                let lhs_vars = self.collect_free_vars(lhs);
                let rhs_vars = self.collect_free_vars(rhs);
                let all_vars: Vec<String> =
                    lhs_vars.iter().chain(rhs_vars.iter()).cloned().collect();

                let lhs_val = self.resolve_value_map(lhs, param_name, arg_value, value_map);
                let rhs_val = self.resolve_value_map(rhs, param_name, arg_value, value_map);

                match (lhs_val, rhs_val) {
                    (Some(l), Some(r)) => {
                        if self.eval_binary_bool(*op, l, r) {
                            ConstraintEvalResult::Satisfied
                        } else {
                            ConstraintEvalResult::Violated(format!(
                                "{} {} {} is false",
                                l,
                                self.fmt_op(*op),
                                r
                            ))
                        }
                    }
                    _ if !all_vars.is_empty() => ConstraintEvalResult::NeedsPropagation(all_vars),
                    _ => ConstraintEvalResult::NeedsPropagation(vec![]),
                }
            }
            _ => ConstraintEvalResult::NeedsPropagation(vec![]),
        }
    }

    fn resolve_value_map(
        &self,
        expr: &Expr,
        param_name: &str,
        arg_value: Option<i64>,
        value_map: &HashMap<String, Option<i64>>,
    ) -> Option<i64> {
        match expr {
            Expr::Integer(n, _) => Some(*n),
            Expr::Ident(id) => {
                if id.name == param_name {
                    arg_value
                } else if let Some(Some(val)) = value_map.get(&id.name) {
                    Some(*val)
                } else if let Some(None) = value_map.get(&id.name) {
                    None
                } else {
                    self.constants.get(&id.name).copied()
                }
            }
            Expr::Binary { lhs, op, rhs, .. } => {
                let lhs_val = self.resolve_value_map(lhs, param_name, arg_value, value_map)?;
                let rhs_val = self.resolve_value_map(rhs, param_name, arg_value, value_map)?;
                self.eval_binary_constraint(*op, lhs_val, rhs_val)
            }
            _ => None,
        }
    }

    fn fmt_constraint_with_value_map(
        &self,
        constraint: &Expr,
        param_name: &str,
        arg_val: i64,
        value_map: &HashMap<String, Option<i64>>,
    ) -> String {
        match constraint {
            Expr::Binary { lhs, op, rhs, .. } => {
                let lhs_str = self.fmt_expr_with_value_map(lhs, param_name, arg_val, value_map);
                let rhs_str = self.fmt_expr_with_value_map(rhs, param_name, arg_val, value_map);
                format!("{} {} {}", lhs_str, self.fmt_op(*op), rhs_str)
            }
            _ => "constraint evaluation failed".to_string(),
        }
    }

    fn fmt_expr_with_value_map(
        &self,
        expr: &Expr,
        param_name: &str,
        arg_val: i64,
        value_map: &HashMap<String, Option<i64>>,
    ) -> String {
        match expr {
            Expr::Integer(n, _) => n.to_string(),
            Expr::Ident(id) => {
                if id.name == param_name {
                    format!("{} ({})", param_name, arg_val)
                } else if let Some(Some(val)) = value_map.get(&id.name) {
                    format!("{} ({})", id.name, val)
                } else if let Some(None) = value_map.get(&id.name) {
                    format!("{} (?)", id.name)
                } else {
                    self.constants
                        .get(&id.name)
                        .map(|v| format!("{} ({})", id.name, v))
                        .unwrap_or_else(|| id.name.clone())
                }
            }
            _ => "expr".to_string(),
        }
    }

    fn collect_free_vars(&self, expr: &Expr) -> Vec<String> {
        let mut vars = Vec::new();
        self.collect_free_vars_rec(expr, &mut vars);
        vars
    }

    fn collect_free_vars_rec(&self, expr: &Expr, vars: &mut Vec<String>) {
        match expr {
            Expr::Ident(id) => {
                // Check both constants AND locals (function parameters)
                if !self.constants.contains_key(&id.name)
                    && !self.locals.contains_key(&id.name)
                    && !vars.contains(&id.name)
                {
                    vars.push(id.name.clone());
                }
            }
            Expr::Binary { lhs, rhs, .. } => {
                self.collect_free_vars_rec(lhs, vars);
                self.collect_free_vars_rec(rhs, vars);
            }
            Expr::Call { func, args, .. } => {
                self.collect_free_vars_rec(func, vars);
                for arg in args {
                    self.collect_free_vars_rec(arg, vars);
                }
            }
            _ => {}
        }
    }

    fn fmt_constraint_with_values(
        &self,
        constraint: &Expr,
        param_name: &str,
        arg_val: i64,
        param_to_arg: &HashMap<String, &Expr>,
    ) -> String {
        match constraint {
            Expr::Binary { lhs, op, rhs, .. } => {
                let lhs_str = self.fmt_expr_with_value(lhs, param_name, arg_val, param_to_arg);
                let rhs_str = self.fmt_expr_with_value(rhs, param_name, arg_val, param_to_arg);
                format!("{} {} {}", lhs_str, self.fmt_op(*op), rhs_str)
            }
            _ => "constraint evaluation failed".to_string(),
        }
    }

    fn fmt_expr_with_value(
        &self,
        expr: &Expr,
        param_name: &str,
        arg_val: i64,
        param_to_arg: &HashMap<String, &Expr>,
    ) -> String {
        match expr {
            Expr::Integer(n, _) => n.to_string(),
            Expr::Ident(id) => {
                if id.name == param_name {
                    format!("{} ({})", param_name, arg_val)
                } else if let Some(arg_expr) = param_to_arg.get(&id.name) {
                    let val = self.const_value(arg_expr);
                    match val {
                        Some(v) => format!("{} ({})", id.name, v),
                        None => {
                            let arg_name = self.get_arg_name(arg_expr);
                            format!("{} ({})", id.name, arg_name)
                        }
                    }
                } else {
                    self.constants
                        .get(&id.name)
                        .map(|v| format!("{} ({})", id.name, v))
                        .unwrap_or_else(|| id.name.clone())
                }
            }
            _ => "expr".to_string(),
        }
    }

    fn fmt_op(&self, op: kettu_parser::BinOp) -> String {
        match op {
            kettu_parser::BinOp::Add => "+".to_string(),
            kettu_parser::BinOp::Sub => "-".to_string(),
            kettu_parser::BinOp::Mul => "*".to_string(),
            kettu_parser::BinOp::Div => "/".to_string(),
            kettu_parser::BinOp::Eq => "==".to_string(),
            kettu_parser::BinOp::Ne => "!=".to_string(),
            kettu_parser::BinOp::Lt => "<".to_string(),
            kettu_parser::BinOp::Le => "<=".to_string(),
            kettu_parser::BinOp::Gt => ">".to_string(),
            kettu_parser::BinOp::Ge => ">=".to_string(),
            kettu_parser::BinOp::And => "&&".to_string(),
            kettu_parser::BinOp::Or => "||".to_string(),
        }
    }

    fn is_in_test_function(&self) -> bool {
        self.in_test_function
    }

    fn check_hush_comment(&self, span: &Span, error_msg: &str) -> bool {
        let span_line = self.byte_offset_to_line(span.start);
        let span_col_start = self.byte_offset_to_col(span.start);
        let span_col_end = self.byte_offset_to_col(span.end.saturating_sub(1));

        for comment in &self.hush_comments {
            if let Expr::String(expected_text, _) = &comment.constraint {
                if expected_text != error_msg {
                    continue;
                }

                // Hush comment must be on the line after the error's line
                if comment.line != span_line + 1 {
                    continue;
                }

                // The ^ column should point within the error span's column range
                // Allow some tolerance for whitespace alignment
                let caret = comment.col;
                if caret >= span_col_start.saturating_sub(2) && caret <= span_col_end + 2 {
                    return true;
                }
            }
        }
        false
    }

    fn byte_offset_to_line(&self, offset: usize) -> usize {
        match self.source_line_offsets.binary_search(&offset) {
            Ok(line) => line,
            Err(insert_pos) => {
                if insert_pos == 0 {
                    0
                } else {
                    insert_pos - 1
                }
            }
        }
    }

    fn byte_offset_to_col(&self, offset: usize) -> usize {
        let line = self.byte_offset_to_line(offset);
        if line < self.source_line_offsets.len() {
            offset - self.source_line_offsets[line]
        } else {
            0
        }
    }

    fn compute_inherited_constraints(
        &self,
        iface: &Interface,
        function_constraints: &HashMap<String, Vec<ParamConstraint>>,
        function_params: &HashMap<String, Vec<String>>,
    ) -> HashMap<String, Vec<InheritedConstraint>> {
        let mut result = HashMap::new();

        for item in &iface.items {
            if let InterfaceItem::Func(func) = item {
                if let Some(body) = &func.body {
                    let func_name = &func.name.name;
                    let func_param_names: Vec<String> =
                        func.params.iter().map(|p| p.name.name.clone()).collect();

                    let local_constants = Self::collect_body_constants(&body.statements);

                    let mut inherited = Vec::new();
                    Self::find_inherited_in_stmts(
                        &body.statements,
                        &func_param_names,
                        &local_constants,
                        function_constraints,
                        function_params,
                        func_name,
                        &mut inherited,
                    );

                    if !inherited.is_empty() {
                        result.insert(func_name.clone(), inherited);
                    }
                }
            }
        }

        result
    }

    fn collect_body_constants(stmts: &[Statement]) -> HashMap<String, i64> {
        let mut constants = HashMap::new();
        for stmt in stmts {
            if let Statement::Let { name, value } = stmt {
                if let Some(val) = Self::const_value_static(value, &constants) {
                    constants.insert(name.name.clone(), val);
                }
            }
        }
        constants
    }

    fn const_value_static(expr: &Expr, constants: &HashMap<String, i64>) -> Option<i64> {
        match expr {
            Expr::Integer(n, _) => Some(*n),
            Expr::Ident(id) => constants.get(&id.name).copied(),
            Expr::Neg(inner, _) => Self::const_value_static(inner, constants).map(|v| -v),
            Expr::Binary { lhs, op, rhs, .. } => {
                let l = Self::const_value_static(lhs, constants)?;
                let r = Self::const_value_static(rhs, constants)?;
                match op {
                    BinOp::Add => Some(l + r),
                    BinOp::Sub => Some(l - r),
                    BinOp::Mul => Some(l * r),
                    BinOp::Div if r != 0 => Some(l / r),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn find_inherited_in_stmts(
        stmts: &[Statement],
        func_param_names: &[String],
        local_constants: &HashMap<String, i64>,
        function_constraints: &HashMap<String, Vec<ParamConstraint>>,
        function_params: &HashMap<String, Vec<String>>,
        current_func: &str,
        result: &mut Vec<InheritedConstraint>,
    ) {
        for stmt in stmts {
            match stmt {
                Statement::Expr(expr) => {
                    Self::find_inherited_in_expr(
                        expr,
                        func_param_names,
                        local_constants,
                        function_constraints,
                        function_params,
                        current_func,
                        result,
                    );
                }
                Statement::Let { value, .. } => {
                    Self::find_inherited_in_expr(
                        value,
                        func_param_names,
                        local_constants,
                        function_constraints,
                        function_params,
                        current_func,
                        result,
                    );
                }
                Statement::Return(Some(expr)) => {
                    Self::find_inherited_in_expr(
                        expr,
                        func_param_names,
                        local_constants,
                        function_constraints,
                        function_params,
                        current_func,
                        result,
                    );
                }
                Statement::Guard {
                    condition,
                    else_body,
                } => {
                    Self::find_inherited_in_expr(
                        condition,
                        func_param_names,
                        local_constants,
                        function_constraints,
                        function_params,
                        current_func,
                        result,
                    );
                    Self::find_inherited_in_stmts(
                        else_body,
                        func_param_names,
                        local_constants,
                        function_constraints,
                        function_params,
                        current_func,
                        result,
                    );
                }
                Statement::GuardLet { else_body, .. } => {
                    Self::find_inherited_in_stmts(
                        else_body,
                        func_param_names,
                        local_constants,
                        function_constraints,
                        function_params,
                        current_func,
                        result,
                    );
                }
                _ => {}
            }
        }
    }

    fn find_inherited_in_expr(
        expr: &Expr,
        func_param_names: &[String],
        local_constants: &HashMap<String, i64>,
        function_constraints: &HashMap<String, Vec<ParamConstraint>>,
        function_params: &HashMap<String, Vec<String>>,
        current_func: &str,
        result: &mut Vec<InheritedConstraint>,
    ) {
        match expr {
            Expr::Call {
                func: callee, args, ..
            } => {
                if let Expr::Ident(id) = callee.as_ref() {
                    let callee_name = &id.name;
                    if callee_name != current_func {
                        if let Some(constraints) = function_constraints.get(callee_name) {
                            let callee_params = function_params
                                .get(callee_name)
                                .cloned()
                                .unwrap_or_default();

                            let mut target_param_sources = HashMap::new();
                            for (i, param_name) in callee_params.iter().enumerate() {
                                if i < args.len() {
                                    let arg = &args[i];
                                    let source =
                                        Self::classify_arg(arg, func_param_names, local_constants);
                                    target_param_sources.insert(param_name.clone(), source);
                                }
                            }

                            for constraint in constraints {
                                result.push(InheritedConstraint {
                                    target_func: callee_name.clone(),
                                    via: vec![current_func.to_string()],
                                    constraint: constraint.clone(),
                                    target_param_sources: target_param_sources.clone(),
                                });
                            }
                        }
                    }
                }

                for arg in args {
                    Self::find_inherited_in_expr(
                        arg,
                        func_param_names,
                        local_constants,
                        function_constraints,
                        function_params,
                        current_func,
                        result,
                    );
                }
            }
            Expr::Binary { lhs, rhs, .. } => {
                Self::find_inherited_in_expr(
                    lhs,
                    func_param_names,
                    local_constants,
                    function_constraints,
                    function_params,
                    current_func,
                    result,
                );
                Self::find_inherited_in_expr(
                    rhs,
                    func_param_names,
                    local_constants,
                    function_constraints,
                    function_params,
                    current_func,
                    result,
                );
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                Self::find_inherited_in_expr(
                    cond,
                    func_param_names,
                    local_constants,
                    function_constraints,
                    function_params,
                    current_func,
                    result,
                );
                Self::find_inherited_in_stmts(
                    then_branch,
                    func_param_names,
                    local_constants,
                    function_constraints,
                    function_params,
                    current_func,
                    result,
                );
                if let Some(else_stmts) = else_branch {
                    Self::find_inherited_in_stmts(
                        else_stmts,
                        func_param_names,
                        local_constants,
                        function_constraints,
                        function_params,
                        current_func,
                        result,
                    );
                }
            }
            Expr::Neg(inner, _) | Expr::Not(inner, _) => {
                Self::find_inherited_in_expr(
                    inner,
                    func_param_names,
                    local_constants,
                    function_constraints,
                    function_params,
                    current_func,
                    result,
                );
            }
            _ => {}
        }
    }

    fn classify_arg(
        arg: &Expr,
        func_param_names: &[String],
        local_constants: &HashMap<String, i64>,
    ) -> ParamSource {
        match arg {
            Expr::Ident(id) => {
                if func_param_names.contains(&id.name) {
                    ParamSource::Param(id.name.clone())
                } else if let Some(val) = local_constants.get(&id.name) {
                    ParamSource::Constant(*val)
                } else {
                    ParamSource::Param(id.name.clone())
                }
            }
            Expr::Integer(n, _) => ParamSource::Constant(*n),
            Expr::Neg(inner, _) => {
                match Self::classify_arg(inner, func_param_names, local_constants) {
                    ParamSource::Constant(v) => ParamSource::Constant(-v),
                    other => other,
                }
            }
            Expr::Binary { lhs, op, rhs, .. } => {
                let l = Self::classify_arg(lhs, func_param_names, local_constants);
                let r = Self::classify_arg(rhs, func_param_names, local_constants);
                match (l, r) {
                    (ParamSource::Constant(lv), ParamSource::Constant(rv)) => {
                        let val = match op {
                            BinOp::Add => Some(lv + rv),
                            BinOp::Sub => Some(lv - rv),
                            BinOp::Mul => Some(lv * rv),
                            BinOp::Div if rv != 0 => Some(lv / rv),
                            _ => None,
                        };
                        match val {
                            Some(v) => ParamSource::Constant(v),
                            None => ParamSource::Param(format!("{:?}", arg)),
                        }
                    }
                    _ => ParamSource::Param(format!("{:?}", arg)),
                }
            }
            _ => ParamSource::Param(format!("{:?}", arg)),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use kettu_parser::parse_file;

    #[test]
    fn test_unknown_type_error() {
        let source = r#"
            package local:test;
            
            interface foo {
                bar: func(x: unknown-type);
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        assert!(!diags.is_empty(), "Should have diagnostics");
        assert_eq!(diags[0].code, DiagnosticCode::UnknownType);
    }

    #[test]
    fn test_valid_types() {
        let source = r#"
            package local:test;
            
            interface foo {
                record my-record {
                    x: u32,
                }
                
                bar: func(x: my-record) -> string;
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        assert!(diags.is_empty(), "Should have no errors: {:?}", diags);
    }

    #[test]
    fn test_duplicate_type() {
        let source = r#"
            package local:test;
            
            interface foo {
                record bar { x: u32, }
                record bar { y: u32, }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        assert!(!diags.is_empty(), "Should have duplicate error");
        assert_eq!(diags[0].code, DiagnosticCode::DuplicateDefinition);
    }

    #[test]
    fn test_nested_packages_reported() {
        let span = 0..1;
        let root = WitFile {
            package: Some(PackageDecl {
                path: PackagePath {
                    namespace: vec![Id::new("local", span.clone())],
                    name: vec![Id::new("root", span.clone())],
                    version: None,
                },
                span: span.clone(),
            }),
            items: vec![TopLevelItem::NestedPackage(NestedPackage {
                path: PackagePath {
                    namespace: vec![Id::new("nested", span.clone())],
                    name: vec![Id::new("demo", span.clone())],
                    version: Some(Version {
                        major: 1,
                        minor: 2,
                        patch: 3,
                        prerelease: None,
                        span: span.clone(),
                    }),
                },
                items: vec![],
                span: span.clone(),
            })],
        };

        let diags = check(&root);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();

        assert!(
            errors
                .iter()
                .any(|d| d.code == DiagnosticCode::NestedPackageUnsupported),
            "Should flag nested packages as unsupported: {:?}",
            errors
        );
    }

    #[test]
    fn test_unknown_variable() {
        let source = r#"
            package local:test;
            
            interface ops {
                compute: func(x: s32) -> s32 {
                    return y;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        assert!(!diags.is_empty(), "Should have unknown variable error");
        assert_eq!(diags[0].code, DiagnosticCode::UnknownVariable);
    }

    #[test]
    fn test_valid_expression() {
        let source = r#"
            package local:test;
            
            interface ops {
                compute: func(x: s32) -> s32 {
                    let y = x + 1;
                    return y;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        // No errors expected - all variables are in scope
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "Should have no errors: {:?}", errors);
    }

    #[test]
    fn test_deprecated_function() {
        let source = r#"
            package local:test;
            
            @since(version = 1.0.0)
            interface versioned {
                @deprecated(version = 2.0.0)
                old-func: func();
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        // Should have a deprecation warning
        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .collect();
        assert!(!warnings.is_empty(), "Should have deprecation warning");
        assert_eq!(warnings[0].code, DiagnosticCode::DeprecatedFeature);
    }

    #[test]
    fn test_return_type_mismatch() {
        let source = r#"
            package local:test;
            
            interface foo {
                bar: func() -> s32 {
                    return true;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        // Should have a return type mismatch error
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "Should have type mismatch error");
        assert!(
            errors[0].message.contains("mismatch"),
            "Error should mention mismatch: {}",
            errors[0].message
        );
    }

    #[test]
    fn test_qualified_variant_literal_valid() {
        let source = r#"
            package local:test;

            interface foo {
                variant my-result {
                    ok(s32),
                    err(string),
                }

                bar: func() {
                    let v = my-result#ok(42);
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "Should have no errors: {:?}", errors);
    }

    #[test]
    fn test_qualified_variant_literal_unknown_case() {
        let source = r#"
            package local:test;

            interface foo {
                variant my-result {
                    ok(s32),
                    err(string),
                }

                bar: func() {
                    let v = my-result#nope(42);
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "Should have a variant case error");
        assert!(
            errors.iter().any(|d| d
                .message
                .contains("Unknown case 'nope' in variant type 'my-result'")),
            "Should report unknown qualified variant case: {:?}",
            errors
        );
    }

    #[test]
    fn test_qualified_variant_literal_unknown_type() {
        let source = r#"
            package local:test;

            interface foo {
                bar: func() {
                    let v = missing-type#ok(42);
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "Should have unknown type error");
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("Unknown type: missing-type")),
            "Should report unknown qualified variant type: {:?}",
            errors
        );
    }

    #[test]
    fn test_qualified_variant_literal_missing_required_payload() {
        let source = r#"
            package local:test;

            interface foo {
                variant my-result {
                    ok(s32),
                    err,
                }

                bar: func() {
                    let v = my-result#ok();
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "Should have payload arity error");
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("Case 'my-result#ok' requires a payload")),
            "Should report missing payload for qualified variant case: {:?}",
            errors
        );
    }

    #[test]
    fn test_qualified_variant_literal_rejects_unexpected_payload() {
        let source = r#"
            package local:test;

            interface foo {
                variant switch {
                    on,
                    off,
                }

                bar: func() {
                    let v = switch#on(1);
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "Should have payload arity error");
        assert!(
            errors.iter().any(|d| d
                .message
                .contains("Case 'switch#on' does not accept a payload")),
            "Should report unexpected payload for qualified variant case: {:?}",
            errors
        );
    }

    #[test]
    fn test_qualified_match_pattern_unknown_type() {
        let source = r#"
            package local:test;

            interface foo {
                variant my-result {
                    ok(s32),
                    err,
                }

                bar: func() -> bool {
                    let r = my-result#ok(42);
                    return match r {
                        missing#ok(v) => true,
                        _ => false,
                    };
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "Should have unknown type error");
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("Unknown type: missing")),
            "Should report unknown qualified pattern type: {:?}",
            errors
        );
    }

    #[test]
    fn test_qualified_match_pattern_unknown_case() {
        let source = r#"
            package local:test;

            interface foo {
                variant my-result {
                    ok(s32),
                    err,
                }

                bar: func() -> bool {
                    let r = my-result#ok(42);
                    return match r {
                        my-result#nope(v) => true,
                        _ => false,
                    };
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "Should have unknown case error");
        assert!(
            errors.iter().any(|d| d
                .message
                .contains("Unknown case 'nope' in variant type 'my-result'")),
            "Should report unknown qualified pattern case: {:?}",
            errors
        );
    }

    #[test]
    fn test_qualified_match_pattern_requires_binding_for_payload_case() {
        let source = r#"
            package local:test;

            interface foo {
                variant my-result {
                    ok(s32),
                    err,
                }

                bar: func() -> bool {
                    let r = my-result#ok(42);
                    return match r {
                        my-result#ok => true,
                        _ => false,
                    };
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            !errors.is_empty(),
            "Should have pattern binding arity error"
        );
        assert!(
            errors.iter().any(|d| d
                .message
                .contains("Case 'my-result#ok' pattern requires a binding for payload")),
            "Should report missing binding for payload case pattern: {:?}",
            errors
        );
    }

    #[test]
    fn test_qualified_match_pattern_rejects_binding_for_plain_case() {
        let source = r#"
            package local:test;

            interface foo {
                variant switch {
                    on,
                    off,
                }

                bar: func() -> bool {
                    let r = switch#on;
                    return match r {
                        switch#on(v) => true,
                        _ => false,
                    };
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            !errors.is_empty(),
            "Should have pattern binding arity error"
        );
        assert!(
            errors.iter().any(|d| d
                .message
                .contains("Case 'switch#on' pattern must not bind a payload")),
            "Should report unexpected binding for plain case pattern: {:?}",
            errors
        );
    }

    #[test]
    fn test_match_expression_return_type_mismatch_detected() {
        let source = r#"
            package local:test;

            interface option-result-tests {
                test-none: func() -> string {
                    let x = none;
                    return match x {
                        #some(_) => false,
                        #none => true,
                    };
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "Should have return type mismatch error");
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("Return type mismatch")),
            "Should report return type mismatch for match expression: {:?}",
            errors
        );
    }

    #[test]
    fn test_match_expression_return_type_matches_declared_type() {
        let source = r#"
            package local:test;

            interface option-result-tests {
                test-none: func() -> bool {
                    let x = none;
                    return match x {
                        #some(_) => false,
                        #none => true,
                    };
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "Should have no errors: {:?}", errors);
    }

    #[test]
    fn test_if_expression_return_type_mismatch_detected() {
        let source = r#"
            package local:test;

            interface conditionals {
                bad: func() -> string {
                    return if true {
                        false;
                    } else {
                        true;
                    };
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "Should have type mismatch error");
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("Return type mismatch")),
            "Should report return type mismatch for if expression: {:?}",
            errors
        );
    }

    #[test]
    fn test_if_expression_return_type_matches_declared_type() {
        let source = r#"
            package local:test;

            interface conditionals {
                ok: func() -> bool {
                    return if true {
                        false;
                    } else {
                        true;
                    };
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "Should have no errors: {:?}", errors);
    }

    #[test]
    fn test_if_expression_branch_type_mismatch_diagnostic() {
        let source = r#"
            package local:test;

            interface conditionals {
                bad: func() -> bool {
                    return if true {
                        false;
                    } else {
                        1;
                    };
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "Should have if branch mismatch error");
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("If branch type mismatch")),
            "Should report if branch type mismatch: {:?}",
            errors
        );
    }

    #[test]
    fn test_if_expression_branch_types_match_no_branch_error() {
        let source = r#"
            package local:test;

            interface conditionals {
                ok: func() -> bool {
                    return if true {
                        false;
                    } else {
                        true;
                    };
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let branch_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .filter(|d| d.message.contains("If branch type mismatch"))
            .collect();
        assert!(
            branch_errors.is_empty(),
            "Should not have branch mismatch errors"
        );
    }

    #[test]
    fn test_function_call_return_type_mismatch_detected() {
        let source = r#"
            package local:test;

            interface calls {
                helper: func() -> bool {
                    return true;
                }

                bad: func() -> string {
                    return helper();
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "Should have return type mismatch error");
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("Return type mismatch")),
            "Should report return type mismatch for function call: {:?}",
            errors
        );
    }

    #[test]
    fn test_function_call_return_type_matches_declared_type() {
        let source = r#"
            package local:test;

            interface calls {
                helper: func() -> bool {
                    return true;
                }

                ok: func() -> bool {
                    return helper();
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "Should have no errors: {:?}", errors);
    }

    #[test]
    fn test_record_field_access_return_type_mismatch_detected() {
        let source = r#"
            package local:test;

            interface records {
                record point {
                    x: s32,
                    y: s32,
                }

                bad: func(p: point) -> string {
                    return p.x;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "Should have return type mismatch error");
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("Return type mismatch")),
            "Should report return type mismatch for record field access: {:?}",
            errors
        );
    }

    #[test]
    fn test_record_field_access_return_type_matches_declared_type() {
        let source = r#"
            package local:test;

            interface records {
                record point {
                    x: s32,
                    y: s32,
                }

                ok: func(p: point) -> s32 {
                    return p.x;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "Should have no errors: {:?}", errors);
    }

    #[test]
    fn test_index_expression_return_type_mismatch_detected() {
        let source = r#"
            package local:test;

            interface lists {
                bad-index: func(arr: list<s32>) -> string {
                    return arr[0];
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "Should have return type mismatch error");
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("Return type mismatch")),
            "Should report return type mismatch for index expression: {:?}",
            errors
        );
    }

    #[test]
    fn test_index_expression_return_type_matches_declared_type() {
        let source = r#"
            package local:test;

            interface lists {
                ok-index: func(arr: list<s32>) -> s32 {
                    return arr[0];
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "Should have no errors: {:?}", errors);
    }

    #[test]
    fn test_slice_expression_return_type_mismatch_detected() {
        let source = r#"
            package local:test;

            interface lists {
                bad-slice: func(arr: list<s32>) -> list<string> {
                    return arr[0..1];
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "Should have return type mismatch error");
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("Return type mismatch")),
            "Should report return type mismatch for slice expression: {:?}",
            errors
        );
    }

    #[test]
    fn test_slice_expression_return_type_matches_declared_type() {
        let source = r#"
            package local:test;

            interface lists {
                ok-slice: func(arr: list<s32>) -> list<s32> {
                    return arr[0..1];
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "Should have no errors: {:?}", errors);
    }

    #[test]
    fn test_try_expression_return_type_mismatch_detected() {
        let source = r#"
            package local:test;

            interface effects {
                bad-try: func(v: option<s32>) -> string {
                    return v?;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "Should have return type mismatch error");
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("Return type mismatch")),
            "Should report return type mismatch for try expression: {:?}",
            errors
        );
    }

    #[test]
    fn test_try_expression_return_type_matches_declared_type() {
        let source = r#"
            package local:test;

            interface effects {
                ok-try: func(v: option<s32>) -> s32 {
                    return v?;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "Should have no errors: {:?}", errors);
    }

    #[test]
    fn test_await_expression_return_type_mismatch_detected() {
        let source = r#"
            package local:test;

            interface effects {
                bad-await: func(f: future<s32>) -> string {
                    return await f;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "Should have return type mismatch error");
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("Return type mismatch")),
            "Should report return type mismatch for await expression: {:?}",
            errors
        );
    }

    #[test]
    fn test_await_expression_return_type_matches_declared_type() {
        let source = r#"
            package local:test;

            interface effects {
                ok-await: func(f: future<s32>) -> s32 {
                    return await f;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "Should have no errors: {:?}", errors);
    }

    #[test]
    fn test_optional_chain_return_type_mismatch_detected() {
        let source = r#"
            package local:test;

            interface effects {
                record point {
                    x: s32,
                }

                bad-chain: func(p: option<point>) -> option<string> {
                    return p?.x;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "Should have return type mismatch error");
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("Return type mismatch")),
            "Should report return type mismatch for optional chain expression: {:?}",
            errors
        );
    }

    #[test]
    fn test_optional_chain_return_type_matches_declared_type() {
        let source = r#"
            package local:test;

            interface effects {
                record point {
                    x: s32,
                }

                ok-chain: func(p: option<point>) -> option<s32> {
                    return p?.x;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "Should have no errors: {:?}", errors);
    }

    #[test]
    fn test_spawn_returns_thread_id() {
        let source = r#"
            package local:test;
            interface effects {
                go: func() -> s32 {
                    let tid = spawn {
                        atomic.store(0, 1);
                    };
                    0;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "spawn should type-check cleanly: {:?}",
            errors
        );
    }

    #[test]
    fn test_thread_id_arithmetic_rejected() {
        let source = r#"
            package local:test;
            interface effects {
                go: func() -> s32 {
                    let tid = spawn {
                        atomic.store(0, 1);
                    };
                    tid + 1;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            !errors.is_empty(),
            "arithmetic on ThreadId should be rejected"
        );
    }

    #[test]
    fn test_shared_let_type_checks() {
        let source = r#"
            package local:test;
            interface effects {
                counter: func() -> s32 {
                    shared let counter = 0;
                    0;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "shared let should type-check: {:?}",
            errors
        );
    }

    #[test]
    fn test_atomic_block_type_checks() {
        let source = r#"
            package local:test;
            interface effects {
                go: func() -> s32 {
                    atomic {
                        42;
                    };
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "atomic block should type-check: {:?}",
            errors
        );
    }

    #[test]
    fn test_shared_let_and_atomic_block_combined() {
        let source = r#"
            package local:test;
            interface effects {
                inc: func() -> s32 {
                    shared let counter = 0;
                    let tid = spawn {
                        atomic {
                            1;
                        };
                    };
                    0;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "combined shared let + atomic should type-check: {:?}",
            errors
        );
    }

    #[test]
    fn test_atomic_block_empty_body() {
        let source = r#"
            package local:test;
            interface effects {
                noop: func() -> s32 {
                    atomic {};
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "empty atomic block should type-check: {:?}",
            errors
        );
    }

    #[test]
    fn test_thread_join_type_checks() {
        let source = r#"
            package local:test;
            interface effects {
                go: func() -> s32 {
                    let tid = spawn { 1; };
                    thread.join(tid);
                    0;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "thread.join(tid) should type-check: {:?}",
            errors
        );
    }

    #[test]
    fn test_thread_join_rejects_non_thread_id() {
        let source = r#"
            package local:test;
            interface effects {
                go: func() -> s32 {
                    thread.join(42);
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            !errors.is_empty(),
            "thread.join(42) should be rejected — requires ThreadId"
        );
    }

    #[test]
    fn test_spawn_join_combined() {
        let source = r#"
            package local:test;
            interface effects {
                go: func() -> s32 {
                    shared let counter = 0;
                    let tid = spawn {
                        atomic { 1; };
                    };
                    thread.join(tid);
                    0;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "spawn + join + shared let should type-check: {:?}",
            errors
        );
    }

    #[test]
    fn test_guard_statement_type_checks() {
        let source = r#"
            package local:test;
            interface effects {
                classify: func(flag: bool) -> s32 {
                    guard flag else {
                        return 0;
                    };
                    1;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "guard should type-check: {:?}", errors);
    }

    #[test]
    fn test_guard_statement_requires_bool_condition() {
        let source = r#"
            package local:test;
            interface effects {
                classify: func() -> s32 {
                    guard 42 else {
                        return 0;
                    };
                    1;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            !errors.is_empty(),
            "guard with non-bool condition should be rejected"
        );
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("Guard condition requires bool")),
            "expected guard condition error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_guard_statement_requires_exiting_else_block() {
        let source = r#"
            package local:test;
            interface effects {
                classify: func(flag: bool) -> s32 {
                    guard flag else {
                        let fallback = 0;
                    };
                    1;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("must exit the current scope")),
            "expected guard scope-exit error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_guard_statement_allows_continue_inside_loop() {
        let source = r#"
            package local:test;
            interface effects {
                sum: func() -> s32 {
                    let total = 0;
                    for item in [1, 2, 3] {
                        guard item != 2 else {
                            continue;
                        };
                        total += item;
                    };
                    total;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "guard with continue inside a loop should type-check: {:?}",
            errors
        );
    }

    #[test]
    fn test_guard_let_statement_type_checks_and_binds_payload() {
        let source = r#"
            package local:test;
            interface effects {
                unwrap: func(v: option<s32>) -> s32 {
                    guard let value = v else {
                        return 0;
                    };
                    value;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "guard let should type-check and bind payload: {:?}",
            errors
        );
    }

    #[test]
    fn test_guard_let_accepts_result_ok_payload() {
        let source = r#"
            package local:test;
            interface effects {
                unwrap: func(v: result<s32, string>) -> s32 {
                    guard let value = v else {
                        return 0;
                    };
                    value;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "guard let should unwrap result ok payloads: {:?}",
            errors
        );
    }

    #[test]
    fn test_guard_let_requires_option_or_result_source() {
        let source = r#"
            package local:test;
            interface effects {
                unwrap: func(v: s32) -> s32 {
                    guard let value = v else {
                        return 0;
                    };
                    value;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.iter().any(|d| d
                .message
                .contains("Guard let requires option<T> or result<T, E>")),
            "expected guard let source type error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_guard_let_requires_exiting_else_block() {
        let source = r#"
            package local:test;
            interface effects {
                unwrap: func(v: option<s32>) -> s32 {
                    guard let value = v else {
                        let fallback = 0;
                    };
                    value;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.iter().any(|d| d
                .message
                .contains("`guard let` else block must exit the current scope")),
            "expected guard let scope-exit error, got: {:?}",
            errors
        );
    }

    // ========================================================================
    // Contract constraint tests
    // ========================================================================

    #[test]
    fn test_constraint_violation_detected() {
        let source = r#"
            package local:test;
            interface test {
                bounded: func(small: s32, big: s32 where big > small) -> result<bool, string> {
                    result#ok(true)
                }
                caller: func() -> bool {
                    let big = 10;
                    let small = 20;
                    bounded(small, big);
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("does not satisfy the constraint")),
            "expected constraint violation error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_constraint_satisfied_no_error() {
        let source = r#"
            package local:test;
            interface test {
                bounded: func(small: s32, big: s32 where big > small) -> result<bool, string> {
                    result#ok(true)
                }
                caller: func() -> bool {
                    let big = 20;
                    let small = 10;
                    bounded(small, big);
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let constraint_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.code == DiagnosticCode::ConstraintViolation)
            .collect();
        assert!(
            constraint_errors.is_empty(),
            "expected no constraint errors, got: {:?}",
            constraint_errors
        );
    }

    #[test]
    fn test_constraint_less_than_operator() {
        let source = r#"
            package local:test;
            interface test {
                limited: func(count: s32 where count < 10) -> result<bool, string> {
                    result#ok(true)
                }
                caller: func() -> bool {
                    limited(15);
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.code == DiagnosticCode::ConstraintViolation
            })
            .collect();
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("does not satisfy the constraint")),
            "expected constraint violation with < operator, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_constraint_less_than_satisfied() {
        let source = r#"
            package local:test;
            interface test {
                limited: func(count: s32 where count < 10) -> result<bool, string> {
                    result#ok(true)
                }
                caller: func() -> bool {
                    limited(5);
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let constraint_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.code == DiagnosticCode::ConstraintViolation)
            .collect();
        assert!(
            constraint_errors.is_empty(),
            "expected no constraint errors for satisfied < constraint, got: {:?}",
            constraint_errors
        );
    }

    #[test]
    fn test_constraint_with_integer_literal_one_side() {
        let source = r#"
            package local:test;
            interface test {
                positive: func(x: s32 where x > 0) -> result<bool, string> {
                    result#ok(true)
                }
                caller: func() -> bool {
                    positive(0);
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.code == DiagnosticCode::ConstraintViolation
            })
            .collect();
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("does not satisfy the constraint")),
            "expected constraint violation with literal, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_constraint_propagation_with_non_const_arg() {
        let source = r#"
            package local:test;
            interface test {
                bounded: func(small: s32, big: s32 where big > small) -> result<bool, string> {
                    result#ok(true)
                }
                caller: func(x: s32) -> bool {
                    let big = 10;
                    bounded(x, big);
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Warning && d.code == DiagnosticCode::ConstraintPropagation
            })
            .collect();
        assert!(
            warnings
                .iter()
                .any(|d| d.message.contains("may not satisfy")),
            "expected constraint propagation warning, got: {:?}",
            warnings
        );
    }

    #[test]
    fn test_constraint_propagation_guard_let_suppresses() {
        let source = r#"
            package local:test;
            interface test {
                bounded: func(small: s32, big: s32 where big > small) -> result<bool, string> {
                    result#ok(true)
                }
                caller: func(x: s32) -> result<bool, string> {
                    let big = 10;
                    guard let result = bounded(x, big) else {
                        return result#err("fail");
                    };
                    result
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let constraint_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code == DiagnosticCode::ConstraintViolation
                    || d.code == DiagnosticCode::ConstraintPropagation
            })
            .collect();
        assert!(
            constraint_diags.is_empty(),
            "expected no constraint diagnostics inside guard-let, got: {:?}",
            constraint_diags
        );
    }

    #[test]
    fn test_constraint_propagation_non_test_function() {
        let source = r#"
            package local:test;
            interface test {
                bounded: func(small: s32, big: s32 where big > small) -> result<bool, string> {
                    result#ok(true)
                }
                caller: func(x: s32) -> bool {
                    let big = 10;
                    bounded(x, big);
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let constraint_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code == DiagnosticCode::ConstraintViolation
                    || d.code == DiagnosticCode::ConstraintPropagation
            })
            .collect();
        assert!(
            constraint_diags
                .iter()
                .all(|d| d.severity == Severity::Warning),
            "expected warnings (not errors) for propagation in non-test function, got: {:?}",
            constraint_diags
        );
    }

    #[test]
    fn test_hush_comment_matching_exact_text() {
        let source = r#"
            package local:test;
            interface test {
                @test
                bounded: func(small: s32, big: s32 where big > small) -> result<bool, string> {
                    result#ok(true)
                }
                @test
                caller: func() -> bool {
                    let big = 10;
                    let small = 20;
                    bounded(small, big);
                    ///    ^ big does not satisfy the constraint "big (10) > small (20)" on bounded
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check_with_source(&ast, source);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info)
            .collect();
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.code == DiagnosticCode::ConstraintViolation
            })
            .collect();
        assert!(
            !infos.is_empty(),
            "expected info diagnostic for hushed error, got diags: {:?}",
            diags
        );
        assert!(
            errors.is_empty(),
            "expected no error diagnostics for hushed error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_hush_comment_mismatch_still_error() {
        let source = r#"
            package local:test;
            interface test {
                @test
                bounded: func(small: s32, big: s32 where big > small) -> result<bool, string> {
                    result#ok(true)
                }
                @test
                caller: func() -> bool {
                    let big = 10;
                    let small = 20;
                    bounded(small, big);
                    ///    ^ wrong text that does not match
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check_with_source(&ast, source);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.code == DiagnosticCode::ConstraintViolation
            })
            .collect();
        assert!(
            !errors.is_empty(),
            "expected error diagnostic when hush text doesn't match, got diags: {:?}",
            diags
        );
    }

    #[test]
    fn test_hush_comment_only_in_test_function() {
        let source = r#"
            package local:test;
            interface test {
                bounded: func(small: s32, big: s32 where big > small) -> result<bool, string> {
                    result#ok(true)
                }
                caller: func() -> bool {
                    let big = 10;
                    let small = 20;
                    bounded(small, big);
                    ///    ^ big does not satisfy the constraint "big (10) > small (20)" on bounded
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check_with_source(&ast, source);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info)
            .collect();
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.code == DiagnosticCode::ConstraintViolation
            })
            .collect();
        assert!(
            infos.is_empty(),
            "expected no info diagnostics in non-test function, got: {:?}",
            infos
        );
        assert!(
            !errors.is_empty(),
            "expected error diagnostic in non-test function even with hush comment, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_multiple_hush_comments_in_same_function() {
        let source = r#"
            package local:test;
            interface test {
                @test
                bounded: func(small: s32, big: s32 where big > small) -> result<bool, string> {
                    result#ok(true)
                }
                @test
                caller: func() -> bool {
                    let big = 10;
                    let small = 20;
                    bounded(small, big);
                    ///    ^ big does not satisfy the constraint "big (10) > small (20)" on bounded
                    bounded(small, big);
                    ///    ^ big does not satisfy the constraint "big (10) > small (20)" on bounded
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check_with_source(&ast, source);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Info && d.code == DiagnosticCode::ConstraintViolation
            })
            .collect();
        assert_eq!(
            infos.len(),
            2,
            "expected 2 info diagnostics for 2 hushed errors, got: {:?}",
            diags
        );
    }

    #[test]
    fn test_constrained_func_must_return_result() {
        let source = r#"
            package local:test;
            interface test {
                bounded: func(small: s32, big: s32 where big > small) -> bool {
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("must return result")
                    && d.code == DiagnosticCode::ConstraintViolation),
            "expected error about constrained function needing Result return type, got: {:?}",
            diags
        );
    }

    #[test]
    fn test_constrained_func_returns_result_ok() {
        let source = r#"
            package local:test;
            interface test {
                bounded: func(small: s32, big: s32 where big > small) -> result<bool, string> {
                    result#ok(true)
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let constraint_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.code == DiagnosticCode::ConstraintViolation)
            .collect();
        assert!(
            constraint_errors.is_empty(),
            "expected no errors for constrained func returning result, got: {:?}",
            constraint_errors
        );
    }

    #[test]
    fn test_result_ok_infers_function_return_type() {
        let source = r#"
            package local:test;
            interface test {
                bounded: func(small: s32, big: s32 where big > small) -> result<bool, string> {
                    return result#ok(true);
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let type_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.code == DiagnosticCode::TypeMismatch)
            .collect();
        assert!(
            type_errors.is_empty(),
            "expected no type mismatch for result#ok in result-returning func, got: {:?}",
            type_errors
        );
    }

    #[test]
    fn test_constraint_violation_with_swapped_args() {
        let source = r#"
            package local:test;
            interface test {
                bounded: func(small: s32, big: s32 where big > small) -> result<bool, string> {
                    result#ok(true)
                }
                caller: func() -> bool {
                    let a = 10;
                    let b = 20;
                    bounded(b, a);
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.code == DiagnosticCode::ConstraintViolation
            })
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "expected exactly one constraint violation for swapped args, got: {:?}",
            errors
        );
        assert!(
            errors[0]
                .message
                .contains("does not satisfy the constraint"),
            "error message should mention constraint violation: {}",
            errors[0].message
        );
    }

    #[test]
    fn test_constraint_with_local_constant_chain() {
        let source = r#"
            package local:test;
            interface test {
                positive: func(x: s32 where x > 0) -> result<bool, string> {
                    result#ok(true)
                }
                caller: func() -> bool {
                    let val = -5;
                    let copy = val;
                    positive(copy);
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.code == DiagnosticCode::ConstraintViolation
            })
            .collect();
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("does not satisfy the constraint")),
            "expected constraint violation through local constant chain, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_hush_comment_in_test_helper() {
        let source = r#"
            package local:test;
            interface test {
                bounded: func(small: s32, big: s32 where big > small) -> result<bool, string> {
                    result#ok(true)
                }
                @test-helper
                helper: func(x: s32) -> result<bool, string> {
                    let big = 10;
                    bounded(x, big);
                    ///    ^ big may not satisfy the constraint "big (10) > small (x)" because x is an unconstrained parameter, bounded must be called with a guard
                    guard let r = bounded(x, big) else {
                        return result#err("fail");
                    };
                    r
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check_with_source(&ast, source);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info)
            .collect();
        assert!(
            !infos.is_empty(),
            "expected info diagnostic for hushed error in @test-helper, got diags: {:?}",
            diags
        );
    }

    #[test]
    fn test_transitive_constraint_propagation() {
        let source = r#"
            package local:test;
            interface test {
                bounded: func(small: s32, big: s32 where big > small) -> result<bool, string> {
                    result#ok(true)
                }
                caller: func(x: s32) -> result<bool, string> {
                    let big = 10;
                    bounded(x, big);
                    result#ok(true)
                }
                outer: func() -> bool {
                    let v = 10;
                    caller(v);
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.code == DiagnosticCode::ConstraintViolation
            })
            .collect();
        assert!(
            errors.iter().any(|d| d.message.contains("via caller")),
            "expected transitive constraint violation with 'via caller', got: {:?}",
            errors
        );
    }

    #[test]
    fn test_transitive_constraint_satisfied() {
        let source = r#"
            package local:test;
            interface test {
                bounded: func(small: s32, big: s32 where big > small) -> result<bool, string> {
                    result#ok(true)
                }
                caller: func(x: s32) -> result<bool, string> {
                    let big = 20;
                    bounded(x, big);
                    result#ok(true)
                }
                outer: func() -> bool {
                    let v = 10;
                    caller(v);
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check(&ast);
        let constraint_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.code == DiagnosticCode::ConstraintViolation)
            .collect();
        assert!(
            constraint_errors.is_empty(),
            "expected no constraint errors for satisfied transitive constraint, got: {:?}",
            constraint_errors
        );
    }

    #[test]
    fn test_transitive_constraint_with_hush() {
        let source = r#"
            package local:test;
            interface test {
                bounded: func(small: s32, big: s32 where big > small) -> result<bool, string> {
                    result#ok(true)
                }
                caller: func(x: s32) -> result<bool, string> {
                    let big = 10;
                    bounded(x, big);
                    result#ok(true)
                }
                @test
                outer: func() -> bool {
                    let v = 10;
                    caller(v);
                    ///    ^ v does not satisfy the constraint "big (10) > small (10)" on bounded (via caller)
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check_with_source(&ast, source);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info)
            .collect();
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.code == DiagnosticCode::ConstraintViolation
            })
            .collect();
        assert!(
            !infos.is_empty(),
            "expected info diagnostic for hushed transitive constraint, got diags: {:?}",
            diags
        );
        assert!(
            errors.is_empty(),
            "expected no error diagnostics for hushed transitive constraint, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_hush_comment_wrong_column_still_error() {
        let source = r#"
            package local:test;
            interface test {
                @test
                bounded: func(small: s32, big: s32 where big > small) -> result<bool, string> {
                    result#ok(true)
                }
                @test
                caller: func() -> bool {
                    let big = 10;
                    let small = 20;
                    bounded(small, big);
                    ///                          ^ big does not satisfy the constraint "big (10) > small (20)" on bounded
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check_with_source(&ast, source);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.code == DiagnosticCode::ConstraintViolation
            })
            .collect();
        assert!(
            !errors.is_empty(),
            "expected error when ^ column is far from span, got diags: {:?}",
            diags
        );
    }

    #[test]
    fn test_hush_comment_wrong_line_still_error() {
        let source = r#"
            package local:test;
            interface test {
                @test
                bounded: func(small: s32, big: s32 where big > small) -> result<bool, string> {
                    result#ok(true)
                }
                @test
                caller: func() -> bool {
                    let big = 10;
                    let small = 20;
                    bounded(small, big);
                    let x = 1;
                    ///    ^ big does not satisfy the constraint "big (10) > small (20)" on bounded
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check_with_source(&ast, source);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.code == DiagnosticCode::ConstraintViolation
            })
            .collect();
        assert!(
            !errors.is_empty(),
            "expected error when hush comment is on wrong line, got diags: {:?}",
            diags
        );
    }

    #[test]
    fn test_type_alias_constraint_on_param() {
        let source = r#"
            package local:test;
            interface test {
                type length = s32 where it > 0;
                use-length: func(l: length) -> result<bool, string> {
                    result#ok(true)
                }
                @test
                caller: func() -> bool {
                    use-length(-5);
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check_with_source(&ast, source);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.code == DiagnosticCode::ConstraintViolation
            })
            .collect();
        assert!(
            !errors.is_empty(),
            "expected constraint violation from type alias on param, got: {:?}",
            diags
        );
    }

    #[test]
    fn test_type_alias_constraint_satisfied_on_param() {
        let source = r#"
            package local:test;
            interface test {
                type length = s32 where it > 0;
                use-length: func(l: length) -> result<bool, string> {
                    result#ok(true)
                }
                @test
                caller: func() -> bool {
                    use-length(10);
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check_with_source(&ast, source);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.code == DiagnosticCode::ConstraintViolation)
            .collect();
        assert!(
            errors.is_empty(),
            "expected no errors for satisfied type alias constraint, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_type_alias_constraint_propagation() {
        let source = r#"
            package local:test;
            interface test {
                type length = s32 where it > 0;
                use-length: func(l: length) -> result<bool, string> {
                    result#ok(true)
                }
                intermediate: func(x: s32) -> result<bool, string> {
                    use-length(x);
                    result#ok(true)
                }
                @test
                outer: func() -> bool {
                    intermediate(-1);
                    true
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let diags = check_with_source(&ast, source);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.code == DiagnosticCode::ConstraintViolation
            })
            .collect();
        assert!(
            !errors.is_empty(),
            "expected transitive type alias constraint violation, got: {:?}",
            diags
        );
    }
}
