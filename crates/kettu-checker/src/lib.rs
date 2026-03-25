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
    // Expression type errors
    TypeMismatch,
    UnknownVariable,
    InvalidOperator,
    DeprecatedFeature,
    UnstableFeature,
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
}

/// Information about a world
#[derive(Debug, Clone)]
pub struct WorldInfo {
    pub name: String,
    pub span: Span,
    pub imports: Vec<String>,
    pub exports: Vec<String>,
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
            self.collect_top_level_item(item);
        }
    }

    fn collect_top_level_item(&mut self, item: &TopLevelItem) {
        match item {
            TopLevelItem::Interface(iface) => self.collect_interface(iface),
            TopLevelItem::World(world) => self.collect_world(world),
            TopLevelItem::Use(_) => {} // Handled in validation
            TopLevelItem::NestedPackage(pkg) => {
                for nested_item in &pkg.items {
                    self.collect_top_level_item(nested_item);
                }
            }
        }
    }

    fn collect_interface(&mut self, iface: &Interface) {
        let iface_name = iface.name.name.clone();

        // Check for duplicate interface
        if self.interfaces.contains_key(&iface_name) {
            self.diagnostics.push(Diagnostic::error(
                format!("Duplicate interface definition: {}", iface_name),
                iface.span.clone(),
                DiagnosticCode::DuplicateDefinition,
            ));
            return;
        }

        let mut types = Vec::new();
        let mut functions = Vec::new();
        let mut function_returns = HashMap::new();

        for item in &iface.items {
            match item {
                InterfaceItem::TypeDef(typedef) => {
                    let name = self.collect_typedef(typedef, Some(&iface_name));
                    types.push(name);
                }
                InterfaceItem::Func(func) => {
                    functions.push(func.name.name.clone());
                    if let Some(result) = &func.result {
                        function_returns.insert(func.name.name.clone(), self.ty_to_checked(result));
                    }
                }
                InterfaceItem::Use(_) => {} // Handled in validation
            }
        }

        self.interfaces.insert(
            iface_name.clone(),
            InterfaceInfo {
                name: iface_name,
                span: iface.span.clone(),
                types,
                functions,
                function_returns,
            },
        );
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
            self.validate_top_level_item(item);
        }
    }

    fn validate_top_level_item(&mut self, item: &TopLevelItem) {
        match item {
            TopLevelItem::Interface(iface) => self.validate_interface(iface),
            TopLevelItem::World(world) => self.validate_world(world),
            TopLevelItem::Use(use_stmt) => self.validate_top_level_use(use_stmt),
            TopLevelItem::NestedPackage(pkg) => {
                for nested_item in &pkg.items {
                    self.validate_top_level_item(nested_item);
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

        // Validate result type
        let declared_return = if let Some(ty) = &func.result {
            self.validate_type(ty);
            Some(self.ty_to_checked(ty))
        } else {
            None
        };

        // Validate function body (Kettu extension)
        if let Some(body) = &func.body {
            // Clear locals and add parameters
            self.locals.clear();
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
        }
    }

    fn validate_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Let { name, value } => {
                // Check the value expression
                let value_ty = self.check_expr(value);
                // Add to local scope (infer type from value)
                self.locals.insert(name.name.clone(), value_ty);
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
            Statement::Break { condition } | Statement::Continue { condition } => {
                // Check condition if present (should be bool)
                if let Some(cond) = condition {
                    self.check_expr(cond);
                }
            }
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
            Expr::Call { func, args, .. } => {
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
                            CheckedType::Named(name)
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
                    let binding_name = match &arm.pattern {
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
                                Some(id.name.clone())
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };

                    let mut arm_result: Option<CheckedType> = None;
                    // Type check the body
                    for stmt in &arm.body {
                        if let Statement::Expr(e) = stmt {
                            arm_result = Some(self.check_expr(e));
                        } else if let Statement::Return(Some(e)) = stmt {
                            arm_result = Some(self.check_expr(e));
                        } else if let Statement::Return(None) = stmt {
                            arm_result = Some(CheckedType::Unknown);
                        }
                    }

                    if let Some(arm_ty) = arm_result {
                        if arm_ty != CheckedType::Unknown {
                            match &inferred_arm_type {
                                Some(existing) if *existing != arm_ty => {
                                    inferred_arm_type = Some(CheckedType::Unknown);
                                }
                                None => inferred_arm_type = Some(arm_ty),
                                _ => {}
                            }
                        }
                    }

                    // Remove binding from scope after arm
                    if let Some(name) = binding_name {
                        self.locals.remove(&name);
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
                // Type check body statements
                for stmt in body {
                    if let Statement::Expr(e) = stmt {
                        self.check_expr(e);
                    } else if let Statement::Return(Some(e)) = stmt {
                        self.check_expr(e);
                    }
                }
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
                // Add loop variable to scope (as i32)
                let old_var = self.locals.insert(variable.name.clone(), CheckedType::I32);
                // Type check body statements
                for stmt in body {
                    self.validate_statement(stmt);
                }
                // Restore old variable if shadowed
                if let Some(old) = old_var {
                    self.locals.insert(variable.name.clone(), old);
                } else {
                    self.locals.remove(&variable.name);
                }
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
                // Add loop variable to scope (as element type)
                let old_var = self.locals.insert(variable.name.clone(), elem_ty);
                // Type check body statements
                for stmt in body {
                    self.validate_statement(stmt);
                }
                // Restore old variable if shadowed
                if let Some(old) = old_var {
                    self.locals.insert(variable.name.clone(), old);
                } else {
                    self.locals.remove(&variable.name);
                }
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
        // Check if it's a type parameter (e.g., T in `record pair<T>`)
        if self.type_params.contains(name) {
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
        TypeDefKind::Alias { ty, .. } => TypeKind::Alias {
            target: Box::new(ty.clone()),
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
}
