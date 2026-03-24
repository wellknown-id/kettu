//! Grammar module for Kettu/WIT using rust-sitter (Tree Sitter).
//!
//! The grammar is defined by annotating AST types with `#[derive(Rule)]`.
//! Tree Sitter generates the actual parser from these annotations at build time.
//!
//! ## Architecture
//!
//! Leaf types (`KIdent`, `KInt`, `KStr`) are unit structs that define token
//! patterns. Struct/enum fields reference these leaf types via `#[leaf(Type)]`
//! to capture matched text as `String`, `i32`, etc.
//!
//! The CST nodes are converted to semantic AST nodes (in `crate::ast`) by the
//! `convert` module.

use rust_sitter::{Rule, Spanned};

pub mod convert;
mod tests;

// ============================================================================
// Leaf token types (terminal symbols)
// ============================================================================

/// Kebab-case identifier token
#[derive(Debug, Clone, PartialEq, Eq, Rule)]
#[leaf(pattern(r"[%]?[a-zA-Z_][a-zA-Z0-9_-]*"))]
pub struct KIdent;

/// Integer literal token
#[derive(Debug, Clone, PartialEq, Eq, Rule)]
#[leaf(pattern(r"[0-9]+"))]
pub struct KInt;

/// String literal token (double-quoted, no interpolation yet)
#[derive(Debug, Clone, PartialEq, Eq, Rule)]
#[leaf(pattern(r#""[^"]*""#))]
pub struct KStr;

/// Semantic version token (e.g. 0.2.10, 0.3.0-rc-2026-02-09)
#[derive(Debug, Clone, PartialEq, Eq, Rule)]
#[leaf(pattern(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*)?"))]
pub struct KVersion;

// ============================================================================
// Top-level file structure
// ============================================================================

/// Root grammar node — a complete Kettu/WIT source file
#[derive(Debug, Clone, PartialEq, Rule)]
#[language]
#[extras(
    token(re(r"\s+")),
    token(re(r"//[^\n]*")),
    token(re(r"/\*[^*]*\*+(?:[^/*][^*]*\*+)*/"))
)]
#[word(KIdent)]
pub struct WitFile {
    pub package: Option<Spanned<PackageDecl>>,
    pub items: Vec<Spanned<TopLevelItem>>,
}

// ============================================================================
// Package declaration
// ============================================================================

/// `package namespace:name;` or `package namespace:name@1.0.0;`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct PackageDecl {
    #[leaf("package")]
    _kw: (),
    pub path: PackagePath,
    #[leaf(optional(";"))]
    _semi: Option<()>,
}

/// Package path: `namespace:name`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct PackagePath {
    #[leaf(KIdent)]
    pub namespace: Spanned<String>,
    #[leaf(":")]
    _colon: (),
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    pub version: Option<PackagePathVersion>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct PackagePathVersion {
    #[leaf("@")]
    _at: (),
    pub version: Spanned<Version>,
}

/// Semantic version: `1.0.0`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct Version {
    #[leaf(KVersion)]
    pub raw: Spanned<String>,
}

// ============================================================================
// Top-level items
// ============================================================================

/// Top-level item: interface, world, or use statement
#[derive(Debug, Clone, PartialEq, Rule)]
pub enum TopLevelItem {
    Use(Spanned<TopLevelUse>),
    Interface(Spanned<InterfaceDef>),
    World(Spanned<WorldDef>),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct TopLevelUse {
    #[leaf("use")]
    _kw: (),
    pub path: UsePathRef,
    pub alias: Option<TopLevelUseAlias>,
    #[leaf(";")]
    _semi: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct TopLevelUseAlias {
    #[leaf("as")]
    _kw: (),
    #[leaf(KIdent)]
    pub alias: Spanned<String>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub enum UsePathRef {
    PackageQualified(UsePathPackageQualified),
    Local(UsePathLocal),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct UsePathPackageQualified {
    pub package: UsePathPackage,
    #[leaf(KIdent)]
    pub interface: Spanned<String>,
    pub version: Option<UsePathVersion>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct UsePathLocal {
    #[leaf(KIdent)]
    pub interface: Spanned<String>,
    pub version: Option<UsePathVersion>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct UsePathPackage {
    #[leaf(KIdent)]
    pub namespace: Spanned<String>,
    #[leaf(":")]
    _colon: (),
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    #[leaf("/")]
    _slash: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct UsePathVersion {
    #[leaf("@")]
    _at: (),
    pub version: Spanned<Version>,
}

// ============================================================================
// Feature gates
// ============================================================================

/// Feature gate annotation
#[derive(Debug, Clone, PartialEq, Rule)]
pub enum Gate {
    Since(SinceGate),
    Unstable(UnstableGate),
    Deprecated(DeprecatedGate),
    Test(TestGate),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct SinceGate {
    #[leaf("@since")]
    _at: (),
    #[leaf("(")]
    _lp: (),
    #[leaf("version")]
    _vkw: (),
    #[leaf("=")]
    _eq: (),
    pub version: Spanned<Version>,
    #[leaf(")")]
    _rp: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct UnstableGate {
    #[leaf("@unstable")]
    _at: (),
    #[leaf("(")]
    _lp: (),
    #[leaf("feature")]
    _fkw: (),
    #[leaf("=")]
    _eq: (),
    #[leaf(KIdent)]
    pub feature: Spanned<String>,
    #[leaf(")")]
    _rp: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct DeprecatedGate {
    #[leaf("@deprecated")]
    _at: (),
    #[leaf("(")]
    _lp: (),
    #[leaf("version")]
    _vkw: (),
    #[leaf("=")]
    _eq: (),
    pub version: Spanned<Version>,
    #[leaf(")")]
    _rp: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
#[leaf("@test")]
pub struct TestGate;

// ============================================================================
// Interface
// ============================================================================

/// `interface name { items }`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct InterfaceDef {
    pub gates: Vec<Gate>,
    #[leaf("interface")]
    _kw: (),
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    #[leaf("{")]
    _lb: (),
    pub items: Vec<Spanned<InterfaceItem>>,
    #[leaf("}")]
    _rb: (),
}

/// Item within an interface
#[derive(Debug, Clone, PartialEq, Rule)]
pub enum InterfaceItem {
    TypeDef(Spanned<TypeDef>),
    Use(Spanned<UseStatement>),
    Func(Spanned<FuncDef>),
}

// ============================================================================
// Use statement (within interface/world)
// ============================================================================

/// `use interface.{name1, name2 as alias};`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct UseStatement {
    pub gates: Vec<Gate>,
    #[leaf("use")]
    _kw: (),
    pub path: UsePathRef,
    #[leaf(".")]
    _dot: (),
    #[leaf("{")]
    _lb: (),
    #[sep_by(",")]
    pub items: Vec<UseItem>,
    #[leaf(optional(","))]
    _trailing_comma: Option<()>,
    #[leaf("}")]
    _rb: (),
    #[leaf(";")]
    _semi: (),
}

/// Item in a use statement: `name` or `name as alias`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct UseItem {
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    pub alias: Option<AsAlias>,
}

/// `as alias`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct AsAlias {
    #[leaf("as")]
    _kw: (),
    #[leaf(KIdent)]
    pub name: Spanned<String>,
}

// ============================================================================
// Type definitions
// ============================================================================

/// Type definition (record, variant, enum, flags, resource, alias)
#[derive(Debug, Clone, PartialEq, Rule)]
pub enum TypeDef {
    Record(RecordDef),
    Variant(VariantDef),
    Enum(EnumDef),
    Flags(FlagsDef),
    TypeAlias(TypeAliasDef),
    Resource(ResourceDef),
}

/// Optional type parameters: `<T>` or `<T, U>`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct TypeParams {
    #[leaf("<")]
    _la: (),
    #[sep_by(",")]
    #[leaf(KIdent)]
    pub params: Vec<Spanned<String>>,
    #[leaf(optional(","))]
    _trailing_comma: Option<()>,
    #[leaf(">")]
    _ra: (),
}

/// `record name { fields }` or `record name<T> { fields }`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct RecordDef {
    #[leaf("record")]
    _kw: (),
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    pub type_params: Option<TypeParams>,
    #[leaf("{")]
    _lb: (),
    #[sep_by(",")]
    pub fields: Vec<RecordField>,
    #[leaf(optional(","))]
    _trailing_comma: Option<()>,
    #[leaf("}")]
    _rb: (),
}

/// Record field: `name: ty`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct RecordField {
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    #[leaf(":")]
    _colon: (),
    pub ty: Spanned<TyNode>,
}

/// `variant name { cases }`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct VariantDef {
    #[leaf("variant")]
    _kw: (),
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    pub type_params: Option<TypeParams>,
    #[leaf("{")]
    _lb: (),
    #[sep_by(",")]
    pub cases: Vec<VariantCase>,
    #[leaf(optional(","))]
    _trailing_comma: Option<()>,
    #[leaf("}")]
    _rb: (),
}

/// Variant case: `name` or `name(ty)`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct VariantCase {
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    pub payload: Option<VariantPayload>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct VariantPayload {
    #[leaf("(")]
    _lp: (),
    pub ty: Spanned<TyNode>,
    #[leaf(")")]
    _rp: (),
}

/// `enum name { cases }`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct EnumDef {
    #[leaf("enum")]
    _kw: (),
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    #[leaf("{")]
    _lb: (),
    #[sep_by(",")]
    #[leaf(KIdent)]
    pub cases: Vec<Spanned<String>>,
    #[leaf(optional(","))]
    _trailing_comma: Option<()>,
    #[leaf("}")]
    _rb: (),
}

/// `flags name { flags }`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct FlagsDef {
    #[leaf("flags")]
    _kw: (),
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    #[leaf("{")]
    _lb: (),
    #[sep_by(",")]
    #[leaf(KIdent)]
    pub flags: Vec<Spanned<String>>,
    #[leaf(optional(","))]
    _trailing_comma: Option<()>,
    #[leaf("}")]
    _rb: (),
}

/// `type name = ty;` or `type name<T> = ty;`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct TypeAliasDef {
    #[leaf("type")]
    _kw: (),
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    pub type_params: Option<TypeParams>,
    #[leaf("=")]
    _eq: (),
    pub ty: Spanned<TyNode>,
    #[leaf(";")]
    _semi: (),
}

/// Resource definition: `resource name;` or `resource name { methods }`
#[derive(Debug, Clone, PartialEq, Rule)]
pub enum ResourceDef {
    Simple(SimpleResource),
    WithMethods(ResourceWithMethods),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct SimpleResource {
    #[leaf("resource")]
    _kw: (),
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    #[leaf(";")]
    _semi: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ResourceWithMethods {
    #[leaf("resource")]
    _kw: (),
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    #[leaf("{")]
    _lb: (),
    pub methods: Vec<Spanned<ResourceMethod>>,
    #[leaf("}")]
    _rb: (),
}

/// Resource method
#[derive(Debug, Clone, PartialEq, Rule)]
pub enum ResourceMethod {
    Constructor(ConstructorMethod),
    Static(StaticMethod),
    Instance(InstanceMethod),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ConstructorMethod {
    #[leaf("constructor")]
    _kw: (),
    pub params: ParamList,
    pub body: Option<Spanned<FuncBody>>,
    #[leaf(optional(";"))]
    _semi: Option<()>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct StaticMethod {
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    #[leaf(":")]
    _colon: (),
    #[leaf("static")]
    _static: (),
    #[leaf("func")]
    _func: (),
    pub params: ParamList,
    pub result: Option<ResultType>,
    pub body: Option<Spanned<FuncBody>>,
    #[leaf(optional(";"))]
    _semi: Option<()>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct InstanceMethod {
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    #[leaf(":")]
    _colon: (),
    #[leaf("func")]
    _func: (),
    pub params: ParamList,
    pub result: Option<ResultType>,
    pub body: Option<Spanned<FuncBody>>,
    #[leaf(optional(";"))]
    _semi: Option<()>,
}

// ============================================================================
// Types
// ============================================================================

/// Type node
#[derive(Debug, Clone, PartialEq, Rule)]
pub enum TyNode {
    // Primitives
    #[leaf("u8")]
    U8,
    #[leaf("u16")]
    U16,
    #[leaf("u32")]
    U32,
    #[leaf("u64")]
    U64,
    #[leaf("s8")]
    S8,
    #[leaf("s16")]
    S16,
    #[leaf("s32")]
    S32,
    #[leaf("s64")]
    S64,
    #[leaf("f32")]
    F32,
    #[leaf("f64")]
    F64,
    #[leaf("bool")]
    Bool,
    #[leaf("char")]
    Char,
    #[leaf("string")]
    String_,

    // Generic built-in types
    List(ListType),
    Option_(OptionType),
    Result_(ResultTypeDef),
    Tuple(TupleType),
    Future(FutureType),
    Stream(StreamType),

    // Named type (possibly with generic args)
    Named(NamedType),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ListType {
    #[leaf("list")]
    _kw: (),
    #[leaf("<")]
    _la: (),
    pub element: Spanned<Box<TyNode>>,
    #[leaf(">")]
    _ra: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct OptionType {
    #[leaf("option")]
    _kw: (),
    #[leaf("<")]
    _la: (),
    pub inner: Spanned<Box<TyNode>>,
    #[leaf(">")]
    _ra: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ResultTypeDef {
    #[leaf("result")]
    _kw: (),
    pub args: Option<ResultArgs>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ResultArgs {
    #[leaf("<")]
    _la: (),
    pub ok: Spanned<Box<TyNode>>,
    pub err: Option<ResultErrArg>,
    #[leaf(optional(","))]
    _trailing_comma: Option<()>,
    #[leaf(">")]
    _ra: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ResultErrArg {
    #[leaf(",")]
    _comma: (),
    pub ty: Spanned<Box<TyNode>>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct TupleType {
    #[leaf("tuple")]
    _kw: (),
    #[leaf("<")]
    _la: (),
    #[sep_by(",")]
    pub elements: Vec<Spanned<TyNode>>,
    #[leaf(optional(","))]
    _trailing_comma: Option<()>,
    #[leaf(">")]
    _ra: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct FutureType {
    #[leaf("future")]
    _kw: (),
    pub inner: Option<GenericOneArg>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct StreamType {
    #[leaf("stream")]
    _kw: (),
    pub inner: Option<GenericOneArg>,
}

/// `<T>` — single type argument in angle brackets
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct GenericOneArg {
    #[leaf("<")]
    _la: (),
    pub ty: Spanned<Box<TyNode>>,
    #[leaf(">")]
    _ra: (),
}

/// Named type, possibly with generic args: `foo`, `pair<s32>`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct NamedType {
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    pub args: Option<GenericArgs>,
}

/// `<T1, T2, ...>`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct GenericArgs {
    #[leaf("<")]
    _la: (),
    #[sep_by(",")]
    pub args: Vec<Spanned<TyNode>>,
    #[leaf(optional(","))]
    _trailing_comma: Option<()>,
    #[leaf(">")]
    _ra: (),
}

// ============================================================================
// Functions
// ============================================================================

/// Function definition: `name: func(params) -> result { body }`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct FuncDef {
    pub gates: Vec<Gate>,
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    pub type_params: Option<TypeParams>,
    #[leaf(":")]
    _colon: (),
    #[leaf(optional("async"))]
    pub is_async: Option<()>,
    #[leaf("func")]
    _func: (),
    pub params: ParamList,
    pub result: Option<ResultType>,
    pub body: Option<Spanned<FuncBody>>,
    #[leaf(optional(";"))]
    _semi: Option<()>,
}

/// `-> ty`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ResultType {
    #[leaf("->")]
    _arrow: (),
    pub ty: Spanned<TyNode>,
}

/// `(param1: ty1, param2: ty2)`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ParamList {
    #[leaf("(")]
    _lp: (),
    #[sep_by(",")]
    pub params: Vec<Param>,
    #[leaf(optional(","))]
    _trailing_comma: Option<()>,
    #[leaf(")")]
    _rp: (),
}

/// Function parameter: `name: ty`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct Param {
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    #[leaf(":")]
    _colon: (),
    pub ty: Spanned<TyNode>,
}

/// Function body: `{ statements }`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct FuncBody {
    #[leaf("{")]
    _lb: (),
    pub statements: Vec<Spanned<Stmt>>,
    #[leaf("}")]
    _rb: (),
}

// ============================================================================
// Statements
// ============================================================================

/// Statement in a function body
#[derive(Debug, Clone, PartialEq, Rule)]
pub enum Stmt {
    Let(LetStmt),
    Assign(AssignStmt),
    ReturnValue(ReturnValueStmt),
    ReturnVoid(ReturnVoidStmt),
    Break(BreakStmt),
    Continue(ContinueStmt),
    Expr(ExprStmt),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct LetStmt {
    #[leaf("let")]
    _kw: (),
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    #[leaf("=")]
    _eq: (),
    pub value: Spanned<Expr>,
    #[leaf(";")]
    _semi: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct AssignStmt {
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    #[leaf("=")]
    _eq: (),
    pub value: Spanned<Expr>,
    #[leaf(";")]
    _semi: (),
}

/// `return expr;`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ReturnValueStmt {
    #[leaf("return")]
    _kw: (),
    pub value: Spanned<Expr>,
    #[leaf(";")]
    _semi: (),
}

/// `return;`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ReturnVoidStmt {
    #[leaf("return")]
    _kw: (),
    #[leaf(";")]
    _semi: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct BreakStmt {
    #[leaf("break")]
    _kw: (),
    #[leaf(";")]
    _semi: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ContinueStmt {
    #[leaf("continue")]
    _kw: (),
    #[leaf(";")]
    _semi: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ExprStmt {
    pub expr: Spanned<Expr>,
    #[leaf(";")]
    _semi: (),
}

// ============================================================================
// Expressions
// ============================================================================

/// Expression with operator precedence via prec_left
#[derive(Debug, Clone, PartialEq, Rule)]
pub enum Expr {
    // Atoms
    Integer(#[leaf(KInt)] i64),
    String_(#[leaf(KStr)] String),
    #[leaf("true")]
    True,
    #[leaf("false")]
    False,
    Ident(#[leaf(KIdent)] Spanned<String>),

    // Parenthesized
    Parens(ParenExpr),

    // Unary — inlined for precedence resolution
    /// `!expr`
    #[prec_left(7)]
    Not(#[leaf("!")] (), Spanned<Box<Expr>>),
    /// `assert expr`
    #[prec_left(7)]
    Assert(#[leaf("assert")] (), Spanned<Box<Expr>>),
    /// `await expr`
    #[prec_left(7)]
    Await(#[leaf("await")] (), Spanned<Box<Expr>>),

    // Binary operators (precedence from low to high)
    #[prec_left(1)]
    Or(Spanned<Box<Expr>>, #[leaf("||")] (), Spanned<Box<Expr>>),
    #[prec_left(2)]
    And(Spanned<Box<Expr>>, #[leaf("&&")] (), Spanned<Box<Expr>>),
    #[prec_left(3)]
    Eq(Spanned<Box<Expr>>, #[leaf("==")] (), Spanned<Box<Expr>>),
    #[prec_left(3)]
    Ne(Spanned<Box<Expr>>, #[leaf("!=")] (), Spanned<Box<Expr>>),
    #[prec_left(4)]
    Lt(Spanned<Box<Expr>>, #[leaf("<")] (), Spanned<Box<Expr>>),
    #[prec_left(4)]
    Le(Spanned<Box<Expr>>, #[leaf("<=")] (), Spanned<Box<Expr>>),
    #[prec_left(4)]
    Gt(Spanned<Box<Expr>>, #[leaf(">")] (), Spanned<Box<Expr>>),
    #[prec_left(4)]
    Ge(Spanned<Box<Expr>>, #[leaf(">=")] (), Spanned<Box<Expr>>),
    #[prec_left(5)]
    Add(Spanned<Box<Expr>>, #[leaf("+")] (), Spanned<Box<Expr>>),
    #[prec_left(5)]
    Sub(Spanned<Box<Expr>>, #[leaf("-")] (), Spanned<Box<Expr>>),
    #[prec_left(6)]
    Mul(Spanned<Box<Expr>>, #[leaf("*")] (), Spanned<Box<Expr>>),
    #[prec_left(6)]
    Div(Spanned<Box<Expr>>, #[leaf("/")] (), Spanned<Box<Expr>>),

    // Postfix — inlined for precedence resolution
    /// Function call: `expr(args)` — uses CallArgs struct for the `(args)` part
    #[prec_left(10)]
    Call(Spanned<Box<Expr>>, CallArgs),
    /// Trailing closure call: `expr(args) |x| body` or `expr |x| body`
    #[prec_left(10)]
    TrailingCall(Spanned<Box<Expr>>, TrailingClosureArg),
    /// Try operator: `expr?`
    #[prec_left(10)]
    Try(Spanned<Box<Expr>>, #[leaf("?")] ()),
    /// Optional chaining: `expr?.field`
    #[prec_left(10)]
    OptionalChain(
        Spanned<Box<Expr>>,
        #[leaf("?.")] (),
        #[leaf(KIdent)] Spanned<String>,
    ),
    /// Field access: `expr.field`
    #[prec_left(10)]
    Field(
        Spanned<Box<Expr>>,
        #[leaf(".")] (),
        #[leaf(KIdent)] Spanned<String>,
    ),
    /// Index: `expr[index]`
    #[prec_left(10)]
    Index(
        Spanned<Box<Expr>>,
        #[leaf("[")] (),
        Spanned<Box<Expr>>,
        #[leaf("]")] (),
    ),
    /// Slice: `expr[start..end]`
    #[prec_left(10)]
    Slice(
        Spanned<Box<Expr>>,
        #[leaf("[")] (),
        Spanned<Box<Expr>>,
        #[leaf("..")] (),
        Spanned<Box<Expr>>,
        #[leaf("]")] (),
    ),

    // Compound expressions
    If(IfExpr),
    While(WhileExpr),
    For(ForExpr),
    ForEach(ForEachExpr),
    Match(MatchExpr),

    // Built-in functions
    StrLen(StrLenExpr),
    StrEq(StrEqExpr),
    ListLen(ListLenExpr),
    ListSet(ListSetExpr),
    ListPush(ListPushExpr),

    // Lambda
    Lambda(LambdaExpr),

    // Higher-order builtins
    Map(MapExpr),
    Filter(FilterExpr),
    Reduce(ReduceExpr),

    // Literals
    ListLiteral(ListLiteralExpr),
    RecordLiteral(RecordLiteralExpr),
    /// `#case` — variant literal (payload handled via Call if needed)
    VariantLiteral(#[leaf("#")] (), #[leaf(KIdent)] Spanned<String>),
    /// `type#case` — qualified variant literal (payload handled via Call if needed)
    QualifiedVariantLiteral(
        #[leaf(KIdent)] Spanned<String>,
        #[leaf("#")] (),
        #[leaf(KIdent)] Spanned<String>,
    ),

    // Option/Result constructors
    Some_(SomeExpr),
    None_(NoneExpr),
    Ok_(OkExpr),
    Err_(ErrExpr),
}

/// Arguments for a function call: `(arg1, arg2, ...)`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct CallArgs {
    #[leaf("(")]
    _lp: (),
    #[sep_by(",")]
    pub args: Vec<Spanned<Expr>>,
    #[leaf(optional(","))]
    _trailing_comma: Option<()>,
    #[leaf(")")]
    _rp: (),
}

/// Trailing closure argument: `|x| x * 2`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct TrailingClosureArg {
    pub lambda: Spanned<LambdaExpr>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ParenExpr {
    #[leaf("(")]
    _lp: (),
    pub inner: Spanned<Box<Expr>>,
    #[leaf(")")]
    _rp: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct IfExpr {
    #[leaf("if")]
    _kw: (),
    pub condition: Spanned<Box<Expr>>,
    #[leaf("{")]
    _lb: (),
    pub then_body: Vec<Stmt>,
    #[leaf("}")]
    _rb: (),
    pub else_branch: Option<ElseBranch>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ElseBranch {
    #[leaf("else")]
    _kw: (),
    #[leaf("{")]
    _lb: (),
    pub body: Vec<Stmt>,
    #[leaf("}")]
    _rb: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct WhileExpr {
    #[leaf("while")]
    _kw: (),
    pub condition: Spanned<Box<Expr>>,
    #[leaf("{")]
    _lb: (),
    pub body: Vec<Stmt>,
    #[leaf("}")]
    _rb: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ForExpr {
    #[leaf("for")]
    _for: (),
    #[leaf(KIdent)]
    pub variable: Spanned<String>,
    #[leaf("in")]
    _in: (),
    pub start: Spanned<Box<Expr>>,
    pub direction: ForDirection,
    pub end: Spanned<Box<Expr>>,
    pub step: Option<StepClause>,
    #[leaf("{")]
    _lb: (),
    pub body: Vec<Stmt>,
    #[leaf("}")]
    _rb: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub enum ForDirection {
    #[leaf("to")]
    To,
    #[leaf("downto")]
    Downto,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct StepClause {
    #[leaf("step")]
    _kw: (),
    pub value: Spanned<Box<Expr>>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ForEachExpr {
    #[leaf("for")]
    _for: (),
    #[leaf(KIdent)]
    pub variable: Spanned<String>,
    #[leaf("in")]
    _in: (),
    pub collection: Spanned<Box<Expr>>,
    #[leaf("{")]
    _lb: (),
    pub body: Vec<Stmt>,
    #[leaf("}")]
    _rb: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct MatchExpr {
    #[leaf("match")]
    _kw: (),
    pub scrutinee: Spanned<Box<Expr>>,
    #[leaf("{")]
    _lb: (),
    #[sep_by(",")]
    pub arms: Vec<Spanned<MatchArm>>,
    #[leaf(optional(","))]
    _trailing_comma: Option<()>,
    #[leaf("}")]
    _rb: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct MatchArm {
    pub pattern: Spanned<Pattern>,
    #[leaf("=>")]
    _arrow: (),
    pub body: Spanned<Expr>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub enum Pattern {
    VariantPlain(VariantPatternPlain),
    VariantBound(VariantPatternBound),
    QualifiedVariantPlain(QualifiedVariantPatternPlain),
    QualifiedVariantBound(QualifiedVariantPatternBound),
    #[leaf("_")]
    Wildcard,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct VariantPatternPlain {
    #[leaf("#")]
    _hash: (),
    #[leaf(KIdent)]
    pub case_name: Spanned<String>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct VariantPatternBound {
    #[leaf("#")]
    _hash: (),
    #[leaf(KIdent)]
    pub case_name: Spanned<String>,
    #[leaf("(")]
    _lp: (),
    #[leaf(KIdent)]
    pub binding: Spanned<String>,
    #[leaf(")")]
    _rp: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct QualifiedVariantPatternPlain {
    #[leaf(KIdent)]
    pub type_name: Spanned<String>,
    #[leaf("#")]
    _hash: (),
    #[leaf(KIdent)]
    pub case_name: Spanned<String>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct QualifiedVariantPatternBound {
    #[leaf(KIdent)]
    pub type_name: Spanned<String>,
    #[leaf("#")]
    _hash: (),
    #[leaf(KIdent)]
    pub case_name: Spanned<String>,
    #[leaf("(")]
    _lp: (),
    #[leaf(KIdent)]
    pub binding: Spanned<String>,
    #[leaf(")")]
    _rp: (),
}

// Built-in function expressions

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct StrLenExpr {
    #[leaf("str-len")]
    _kw: (),
    #[leaf("(")]
    _lp: (),
    pub expr: Spanned<Box<Expr>>,
    #[leaf(")")]
    _rp: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct StrEqExpr {
    #[leaf("str-eq")]
    _kw: (),
    #[leaf("(")]
    _lp: (),
    pub a: Spanned<Box<Expr>>,
    #[leaf(",")]
    _comma: (),
    pub b: Spanned<Box<Expr>>,
    #[leaf(")")]
    _rp: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ListLenExpr {
    #[leaf("list-len")]
    _kw: (),
    #[leaf("(")]
    _lp: (),
    pub expr: Spanned<Box<Expr>>,
    #[leaf(")")]
    _rp: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ListSetExpr {
    #[leaf("list-set")]
    _kw: (),
    #[leaf("(")]
    _lp: (),
    pub arr: Spanned<Box<Expr>>,
    #[leaf(",")]
    _c1: (),
    pub idx: Spanned<Box<Expr>>,
    #[leaf(",")]
    _c2: (),
    pub val: Spanned<Box<Expr>>,
    #[leaf(")")]
    _rp: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ListPushExpr {
    #[leaf("list-push")]
    _kw: (),
    #[leaf("(")]
    _lp: (),
    pub arr: Spanned<Box<Expr>>,
    #[leaf(",")]
    _comma: (),
    pub val: Spanned<Box<Expr>>,
    #[leaf(")")]
    _rp: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct LambdaExpr {
    #[leaf("|")]
    _lp: (),
    #[sep_by(",")]
    #[leaf(KIdent)]
    pub params: Vec<Spanned<String>>,
    #[leaf(optional(","))]
    _trailing_comma: Option<()>,
    #[leaf("|")]
    _rp: (),
    pub body: Spanned<Box<Expr>>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct MapExpr {
    #[leaf("map")]
    _kw: (),
    #[leaf("(")]
    _lp: (),
    pub list: Spanned<Box<Expr>>,
    pub lambda: Option<MapLambdaArg>,
    #[leaf(")")]
    _rp: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct MapLambdaArg {
    #[leaf(",")]
    _comma: (),
    pub lambda: Spanned<Box<Expr>>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct FilterExpr {
    #[leaf("filter")]
    _kw: (),
    #[leaf("(")]
    _lp: (),
    pub list: Spanned<Box<Expr>>,
    pub lambda: Option<FilterLambdaArg>,
    #[leaf(")")]
    _rp: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct FilterLambdaArg {
    #[leaf(",")]
    _comma: (),
    pub lambda: Spanned<Box<Expr>>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ReduceExpr {
    #[leaf("reduce")]
    _kw: (),
    #[leaf("(")]
    _lp: (),
    pub list: Spanned<Box<Expr>>,
    #[leaf(",")]
    _c1: (),
    pub init: Spanned<Box<Expr>>,
    pub lambda: Option<ReduceLambdaArg>,
    #[leaf(")")]
    _rp: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ReduceLambdaArg {
    #[leaf(",")]
    _comma: (),
    pub lambda: Spanned<Box<Expr>>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ListLiteralExpr {
    #[leaf("[")]
    _lb: (),
    #[sep_by(",")]
    pub elements: Vec<Spanned<Expr>>,
    #[leaf(optional(","))]
    _trailing_comma: Option<()>,
    #[leaf("]")]
    _rb: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct RecordLiteralExpr {
    #[leaf("{")]
    _lb: (),
    #[sep_by(",")]
    pub fields: Vec<RecordFieldInit>,
    #[leaf(optional(","))]
    _trailing_comma: Option<()>,
    #[leaf("}")]
    _rb: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct RecordFieldInit {
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    #[leaf(":")]
    _colon: (),
    pub value: Spanned<Box<Expr>>,
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct SomeExpr {
    #[leaf("some")]
    _kw: (),
    #[leaf("(")]
    _lp: (),
    pub value: Spanned<Box<Expr>>,
    #[leaf(")")]
    _rp: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
#[leaf("none")]
pub struct NoneExpr;

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct OkExpr {
    #[leaf("ok")]
    _kw: (),
    #[leaf("(")]
    _lp: (),
    pub value: Spanned<Box<Expr>>,
    #[leaf(")")]
    _rp: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ErrExpr {
    #[leaf("err")]
    _kw: (),
    #[leaf("(")]
    _lp: (),
    pub value: Spanned<Box<Expr>>,
    #[leaf(")")]
    _rp: (),
}

// ============================================================================
// World
// ============================================================================

/// `world name { items }`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct WorldDef {
    pub gates: Vec<Gate>,
    #[leaf("world")]
    _kw: (),
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    #[leaf("{")]
    _lb: (),
    pub items: Vec<Spanned<WorldItemDecl>>,
    #[leaf("}")]
    _rb: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct WorldItemDecl {
    pub gates: Vec<Gate>,
    pub item: WorldItem,
}

/// Item within a world
#[derive(Debug, Clone, PartialEq, Rule)]
pub enum WorldItem {
    Import(WorldImport),
    Export(WorldExport),
    Include(WorldInclude),
    Use(WorldUse),
}

/// `import name: func(...);` or `import interface-name;`
#[derive(Debug, Clone, PartialEq, Rule)]
pub enum WorldImport {
    Func(ImportFunc),
    Path(ImportPath),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ImportFunc {
    #[leaf("import")]
    _kw: (),
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    #[leaf(":")]
    _colon: (),
    #[leaf("func")]
    _func: (),
    pub params: ParamList,
    pub result: Option<ResultType>,
    #[leaf(";")]
    _semi: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ImportPath {
    #[leaf("import")]
    _kw: (),
    pub path: UsePathRef,
    #[leaf(";")]
    _semi: (),
}

/// `export name: func(...);` or `export interface-name;`
#[derive(Debug, Clone, PartialEq, Rule)]
pub enum WorldExport {
    Func(ExportFunc),
    Path(ExportPath),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ExportFunc {
    #[leaf("export")]
    _kw: (),
    #[leaf(KIdent)]
    pub name: Spanned<String>,
    #[leaf(":")]
    _colon: (),
    #[leaf("func")]
    _func: (),
    pub params: ParamList,
    pub result: Option<ResultType>,
    #[leaf(";")]
    _semi: (),
}

#[derive(Debug, Clone, PartialEq, Rule)]
pub struct ExportPath {
    #[leaf("export")]
    _kw: (),
    pub path: UsePathRef,
    #[leaf(";")]
    _semi: (),
}

/// `include other-world;`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct WorldInclude {
    #[leaf("include")]
    _kw: (),
    pub path: UsePathRef,
    #[leaf(";")]
    _semi: (),
}

/// `use interface.{name1, name2 as alias};`
#[derive(Debug, Clone, PartialEq, Rule)]
pub struct WorldUse {
    #[leaf("use")]
    _kw: (),
    pub path: UsePathRef,
    #[leaf(".")]
    _dot: (),
    #[leaf("{")]
    _lb: (),
    #[sep_by(",")]
    pub items: Vec<UseItem>,
    #[leaf("}")]
    _rb: (),
    #[leaf(";")]
    _semi: (),
}
