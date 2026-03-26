//! AST (Abstract Syntax Tree) definitions for Kettu/WIT.
//!
//! These types represent the parsed structure of a Kettu source file.
//! The AST is designed to be WIT-compatible while supporting Kettu extensions
//! like function bodies in interface declarations.

use std::ops::Range;

/// Source span for error reporting
pub type Span = Range<usize>;

/// A spanned value, containing both the value and its source location
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub fn new(value: T, span: Span) -> Self {
        Self { value, span }
    }
}

/// An identifier (kebab-case name)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Id {
    pub name: String,
    pub span: Span,
}

impl Id {
    pub fn new(name: impl Into<String>, span: Span) -> Self {
        Self {
            name: name.into(),
            span,
        }
    }
}

/// A complete WIT/Kettu file
#[derive(Debug, Clone, PartialEq)]
pub struct WitFile {
    /// Package declaration (required for valid WIT)
    pub package: Option<PackageDecl>,
    /// Top level items: interfaces, worlds, uses, nested packages
    pub items: Vec<TopLevelItem>,
}

/// Package declaration: `package namespace:name@version;`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageDecl {
    pub path: PackagePath,
    pub span: Span,
}

/// Package path: `namespace:name` or `namespace:name@version`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagePath {
    pub namespace: Vec<Id>,
    pub name: Vec<Id>,
    pub version: Option<Version>,
}

/// Semantic version
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub prerelease: Option<String>,
    pub span: Span,
}

/// Top-level item in a WIT file
#[derive(Debug, Clone, PartialEq)]
pub enum TopLevelItem {
    /// `use pkg:name/interface;`
    Use(TopLevelUse),
    /// `interface name { ... }`
    Interface(Interface),
    /// `world name { ... }`
    World(World),
    /// `package name { ... }` (nested package)
    NestedPackage(NestedPackage),
}

/// Top-level use statement
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopLevelUse {
    pub path: UsePath,
    pub alias: Option<Id>,
    pub span: Span,
}

/// Use path: `interface` or `pkg:name/interface@version`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsePath {
    pub package: Option<PackagePath>,
    pub interface: Id,
}

/// Feature gate: @since, @unstable, @deprecated
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    Since {
        version: Version,
    },
    Unstable {
        feature: Id,
    },
    Deprecated {
        version: Version,
    },
    /// Test function marker
    Test,
}

/// Interface definition
#[derive(Debug, Clone, PartialEq)]
pub struct Interface {
    pub gates: Vec<Gate>,
    pub name: Id,
    pub items: Vec<InterfaceItem>,
    pub span: Span,
}

/// Item within an interface
#[derive(Debug, Clone, PartialEq)]
pub enum InterfaceItem {
    /// Type definition (record, variant, enum, flags, resource, type alias)
    TypeDef(TypeDef),
    /// `use other.{types};`
    Use(UseStatement),
    /// `name: func(...);` or `name: func(...) { ... }` (Kettu extension)
    Func(Func),
}

/// Type definition
#[derive(Debug, Clone, PartialEq)]
pub struct TypeDef {
    pub gates: Vec<Gate>,
    pub kind: TypeDefKind,
    pub span: Span,
}

/// Kind of type definition
#[derive(Debug, Clone, PartialEq)]
pub enum TypeDefKind {
    /// `type name = ty;` or `type name<T> = ty;`
    Alias {
        name: Id,
        type_params: Vec<Id>,
        ty: Ty,
    },
    /// `record name { fields }` or `record name<T> { fields }`
    Record {
        name: Id,
        type_params: Vec<Id>,
        fields: Vec<RecordField>,
    },
    /// `variant name { cases }` or `variant name<T> { cases }`
    Variant {
        name: Id,
        type_params: Vec<Id>,
        cases: Vec<VariantCase>,
    },
    /// `enum name { cases }`
    Enum { name: Id, cases: Vec<Id> },
    /// `flags name { flags }`
    Flags { name: Id, flags: Vec<Id> },
    /// `resource name;` or `resource name { methods }`
    Resource {
        name: Id,
        methods: Vec<ResourceMethod>,
    },
}

/// Record field
#[derive(Debug, Clone, PartialEq)]
pub struct RecordField {
    pub name: Id,
    pub ty: Ty,
}

/// Variant case
#[derive(Debug, Clone, PartialEq)]
pub struct VariantCase {
    pub name: Id,
    pub ty: Option<Ty>,
}

/// Resource method
#[derive(Debug, Clone, PartialEq)]
pub enum ResourceMethod {
    /// `name: func(...);`
    Method(Func),
    /// `name: static func(...);`
    Static(Func),
    /// `constructor(...);` or `constructor(...) { body }`
    Constructor {
        params: Vec<Param>,
        result: Option<Ty>,
        body: Option<FuncBody>,
        span: Span,
    },
}

/// Use statement within interface/world
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseStatement {
    pub path: UsePath,
    pub names: Vec<UseItem>,
    pub span: Span,
}

/// Individual item in a use statement
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseItem {
    pub name: Id,
    pub alias: Option<Id>,
}

/// Function declaration
#[derive(Debug, Clone, PartialEq)]
pub struct Func {
    pub gates: Vec<Gate>,
    pub name: Id,
    /// Type parameters for generic functions (e.g., `swap<T>`)
    pub type_params: Vec<Id>,
    pub is_async: bool,
    pub params: Vec<Param>,
    pub result: Option<Ty>,
    /// Function body (Kettu extension - not valid in pure WIT)
    pub body: Option<FuncBody>,
    pub span: Span,
}

/// Function parameter
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: Id,
    pub ty: Ty,
}

/// Function body (Kettu extension)
#[derive(Debug, Clone, PartialEq)]
pub struct FuncBody {
    pub statements: Vec<Statement>,
    pub span: Span,
}

/// Statement in a function body (Kettu extension)
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    /// Expression statement
    Expr(Expr),
    /// `let name = expr;`
    Let { name: Id, value: Expr },
    /// `return expr;`
    Return(Option<Expr>),
    /// `name = expr;` (assignment to existing variable)
    Assign { name: Id, value: Expr },
    /// `break;` or `break if cond;` - exit the innermost loop
    Break { condition: Option<Box<Expr>> },
    /// `continue;` or `continue if cond;` - skip to next iteration
    Continue { condition: Option<Box<Expr>> },
    /// `shared let name = expr;` — allocate shared memory for atomic access
    SharedLet { name: Id, initial_value: Expr },
}

/// Part of an interpolated string
#[derive(Debug, Clone, PartialEq)]
pub enum StringPart {
    /// Literal text: "Hello, "
    Literal(String),
    /// Expression placeholder: {expr}
    Expr(Box<Expr>),
}

/// Expression (Kettu extension)
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Identifier reference
    Ident(Id),
    /// Integer literal
    Integer(i64, Span),
    /// String literal
    String(String, Span),
    /// Interpolated string: `"Hello, {name}!"`
    InterpolatedString(Vec<StringPart>, Span),
    /// Boolean literal
    Bool(bool, Span),
    /// Function call: `name(args)`
    Call {
        func: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    /// Field access: `expr.field`
    Field {
        expr: Box<Expr>,
        field: Id,
        span: Span,
    },
    /// Optional chaining: `expr?.field`
    /// If expr is some(v), evaluates to some(v.field), otherwise none
    OptionalChain {
        expr: Box<Expr>,
        field: Id,
        span: Span,
    },
    /// Try operator: `expr?`
    /// Unwraps some/ok, or early returns none/err
    Try { expr: Box<Expr>, span: Span },
    /// Binary operation: `a + b`
    Binary {
        lhs: Box<Expr>,
        op: BinOp,
        rhs: Box<Expr>,
        span: Span,
    },
    /// If expression: `if cond { then } else { else }`
    If {
        cond: Box<Expr>,
        then_branch: Vec<Statement>,
        else_branch: Option<Vec<Statement>>,
        span: Span,
    },
    /// Assert expression: `assert <expr>` - panics if false
    Assert(Box<Expr>, Span),
    /// Negation: `!expr`
    Not(Box<Expr>, Span),
    /// String length: `str-len(expr)`
    StrLen(Box<Expr>, Span),
    /// String equality: `str-eq(a, b)`
    StrEq(Box<Expr>, Box<Expr>, Span),
    /// List length: `list-len(expr)`
    ListLen(Box<Expr>, Span),
    /// List set: `list-set(arr, idx, val)` - mutate element at index
    ListSet(Box<Expr>, Box<Expr>, Box<Expr>, Span),
    /// List push: `list-push(arr, val)` - return new list with val appended
    ListPush(Box<Expr>, Box<Expr>, Span),
    /// Lambda expression: `|x, y| expr`
    Lambda {
        params: Vec<Id>,
        body: Box<Expr>,
        /// Captured variables from enclosing scope (filled in during capture analysis)
        captures: Vec<Id>,
        span: Span,
    },
    /// Map built-in: `map(arr, |x| expr)` - returns new list with lambda applied to each element
    Map {
        list: Box<Expr>,
        lambda: Box<Expr>, // Must be a Lambda expr
        span: Span,
    },
    /// Filter built-in: `filter(arr, |x| pred)` - returns new list with elements matching predicate
    Filter {
        list: Box<Expr>,
        lambda: Box<Expr>, // Must be a Lambda expr returning bool
        span: Span,
    },
    /// Reduce built-in: `reduce(arr, init, |acc, x| expr)` - fold list to single value
    Reduce {
        list: Box<Expr>,
        init: Box<Expr>,
        lambda: Box<Expr>, // Must be a Lambda expr with 2 params (acc, elem)
        span: Span,
    },
    /// Record literal: `{ field: value, ... }` or `TypeName { field: value, ... }`
    RecordLiteral {
        /// Optional type name for named record construction
        type_name: Option<Id>,
        /// Field assignments
        fields: Vec<(Id, Box<Expr>)>,
        span: Span,
    },
    /// Variant literal: `#case`, `#case(value)`, or `type#case(value)`
    VariantLiteral {
        /// Optional type name for qualified construction (e.g., result#ok)
        type_name: Option<Id>,
        /// Variant case name
        case_name: Id,
        /// Optional payload value
        payload: Option<Box<Expr>>,
        span: Span,
    },
    /// Match expression: `match expr { pattern => body, ... }`
    Match {
        /// The value being matched
        scrutinee: Box<Expr>,
        /// Match arms
        arms: Vec<MatchArm>,
        span: Span,
    },
    /// While loop: `while condition { body }`
    While {
        /// Loop condition
        condition: Box<Expr>,
        /// Loop body statements
        body: Vec<Statement>,
        span: Span,
    },
    /// Range expression: `start to end` or `start downto end` with optional `step N`
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        /// Step value (default 1)
        step: Option<Box<Expr>>,
        /// True for `downto` (descending), false for `to` (ascending)
        descending: bool,
        span: Span,
    },
    /// For loop: `for var in range { body }`
    For {
        /// Loop variable
        variable: Id,
        /// Range to iterate (must be Range expr)
        range: Box<Expr>,
        /// Loop body statements
        body: Vec<Statement>,
        span: Span,
    },
    /// List literal: `[1, 2, 3]`
    ListLiteral { elements: Vec<Expr>, span: Span },
    /// Index access: `arr[i]`
    Index {
        expr: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    /// Slice: `arr[start..end]`
    Slice {
        expr: Box<Expr>,
        start: Box<Expr>,
        end: Box<Expr>,
        span: Span,
    },
    /// For-each loop: `for item in collection { body }`
    ForEach {
        /// Loop variable (element)
        variable: Id,
        /// Collection to iterate (must be a list)
        collection: Box<Expr>,
        /// Loop body statements
        body: Vec<Statement>,
        span: Span,
    },
    /// Await expression: `await expr`
    /// Suspends until the future completes and returns its value
    Await { expr: Box<Expr>, span: Span },
    /// Atomic load: `atomic.load(addr)` - atomically reads i32 from shared memory
    AtomicLoad { addr: Box<Expr>, span: Span },
    /// Atomic store: `atomic.store(addr, value)` - atomically writes i32 to shared memory
    AtomicStore { addr: Box<Expr>, value: Box<Expr>, span: Span },
    /// Atomic add: `atomic.add(addr, value)` - atomically adds and returns old value
    AtomicAdd { addr: Box<Expr>, value: Box<Expr>, span: Span },
    /// Atomic sub: `atomic.sub(addr, value)` - atomically subtracts and returns old value
    AtomicSub { addr: Box<Expr>, value: Box<Expr>, span: Span },
    /// Atomic compare-exchange: `atomic.cmpxchg(addr, expected, new)` - returns old value
    AtomicCmpxchg { addr: Box<Expr>, expected: Box<Expr>, replacement: Box<Expr>, span: Span },
    /// Atomic wait: `atomic.wait(addr, expected, timeout_ns)` - blocks until notified
    AtomicWait { addr: Box<Expr>, expected: Box<Expr>, timeout: Box<Expr>, span: Span },
    /// Atomic notify: `atomic.notify(addr, count)` - wakes waiting threads
    AtomicNotify { addr: Box<Expr>, count: Box<Expr>, span: Span },
    /// Spawn: `spawn { body }` - runs body on a new thread
    Spawn { body: Vec<Statement>, span: Span },
    /// Thread join: `thread.join(tid)` - blocks until spawned thread completes
    ThreadJoin { tid: Box<Expr>, span: Span },
    /// Atomic block: `atomic { stmts }` - sugar for atomic operations on shared vars
    AtomicBlock { body: Vec<Statement>, span: Span },
}

/// Pattern for match arms
#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// Variant pattern: #case or #case(binding)
    Variant {
        /// Optional type name for qualified patterns
        type_name: Option<Id>,
        /// Variant case name
        case_name: Id,
        /// Optional binding for payload
        binding: Option<Id>,
        span: Span,
    },
    /// Wildcard pattern: _
    Wildcard(Span),
    /// Literal pattern (integer or bool)
    Literal(Box<Expr>),
}

/// Match arm: pattern => body
#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    /// Body statements (last is the result)
    pub body: Vec<Statement>,
    pub span: Span,
}

/// Binary operator
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add, // +
    Sub, // -
    Mul, // *
    Div, // /
    Eq,  // ==
    Ne,  // !=
    Lt,  // <
    Le,  // <=
    Gt,  // >
    Ge,  // >=
    And, // &&
    Or,  // ||
}

/// Type
#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    /// Primitive types: u8, u16, u32, u64, s8, s16, s32, s64, f32, f64, bool, char, string
    Primitive(PrimitiveTy, Span),
    /// Named type reference
    Named(Id),
    /// `list<T>` or `list<T, N>`
    List {
        element: Box<Ty>,
        size: Option<u32>,
        span: Span,
    },
    /// `option<T>`
    Option { inner: Box<Ty>, span: Span },
    /// `result<T, E>`, `result<T>`, `result<_, E>`, `result`
    Result {
        ok: Option<Box<Ty>>,
        err: Option<Box<Ty>>,
        span: Span,
    },
    /// `tuple<T1, T2, ...>`
    Tuple { elements: Vec<Ty>, span: Span },
    /// `future<T>` or `future`
    Future { inner: Option<Box<Ty>>, span: Span },
    /// `stream<T>` or `stream`
    Stream { inner: Option<Box<Ty>>, span: Span },
    /// `own<T>` (owned handle)
    Own { resource: Id, span: Span },
    /// `borrow<T>` (borrowed handle)
    Borrow { resource: Id, span: Span },
    /// Generic type instantiation: `pair<s32>`, `container<string>`
    Generic { name: Id, args: Vec<Ty>, span: Span },
}

/// Primitive types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveTy {
    U8,
    U16,
    U32,
    U64,
    S8,
    S16,
    S32,
    S64,
    F32,
    F64,
    Bool,
    Char,
    String,
}

/// World definition
#[derive(Debug, Clone, PartialEq)]
pub struct World {
    pub gates: Vec<Gate>,
    pub name: Id,
    pub items: Vec<WorldItem>,
    pub span: Span,
}

/// Item within a world
#[derive(Debug, Clone, PartialEq)]
pub enum WorldItem {
    /// Type definition
    TypeDef(TypeDef),
    /// Use statement
    Use(UseStatement),
    /// `import name: ...;` or `import interface;`
    Import(ImportExport),
    /// `export name: ...;` or `export interface;`
    Export(ImportExport),
    /// `include world;`
    Include(IncludeStatement),
}

/// Import or export in a world
#[derive(Debug, Clone, PartialEq)]
pub struct ImportExport {
    pub name: Option<Id>,
    pub kind: ImportExportKind,
    pub span: Span,
}

/// Kind of import/export
#[derive(Debug, Clone, PartialEq)]
pub enum ImportExportKind {
    /// Reference to an interface: `import my-interface;`
    Path(UsePath),
    /// Inline function: `export run: func();`
    Func(Func),
    /// Inline interface: `import env: interface { ... }`
    Interface(Vec<InterfaceItem>),
}

/// Include statement in a world
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncludeStatement {
    pub path: UsePath,
    pub with: Vec<IncludeWith>,
    pub span: Span,
}

/// Rename in an include statement
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncludeWith {
    pub from: Id,
    pub to: Id,
}

/// Nested package definition
#[derive(Debug, Clone, PartialEq)]
pub struct NestedPackage {
    pub path: PackagePath,
    pub items: Vec<TopLevelItem>,
    pub span: Span,
}
