//! Kettu LSP Server
//!
//! Language Server Protocol implementation for Kettu/WIT.
//! Provides diagnostics, hover, go-to-definition, document symbols, and completion.

use kettu_checker::{Severity, check};
use kettu_codegen::resolve_imports;
use kettu_parser::{
    BinOp, Expr, ImportExportKind, InterfaceItem, Pattern, PrimitiveTy, Statement, TopLevelItem,
    Ty, TypeDefKind, WitFile, WorldItem, parse_file,
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

// ============================================================================
// Document State
// ============================================================================

/// Cached document state
struct Document {
    content: String,
    #[allow(dead_code)]
    version: i32,
    /// Parsed AST (if successful)
    ast: Option<WitFile>,
}

impl Document {
    fn new(content: String, version: i32) -> Self {
        let (ast, parse_errors) = parse_file(&content);
        let ast = if ast.is_some()
            && !parse_errors.is_empty()
            && has_only_comment_induced_parse_errors(&content)
        {
            parse_clean_without_comments(&content).or(ast)
        } else {
            ast
        };
        Self {
            content,
            version,
            ast,
        }
    }
}

// ============================================================================
// Symbol Index
// ============================================================================

/// A symbol definition with its location
#[derive(Debug, Clone)]
struct SymbolDef {
    name: String,
    kind: SymbolDefKind,
    span: std::ops::Range<usize>,
    /// For nested items, the containing interface/world
    container: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // Some variants reserved for future use
enum SymbolDefKind {
    Interface,
    World,
    Func {
        params: Vec<String>,
        result: Option<String>,
    },
    Type {
        description: String,
    },
    Record,
    Enum,
    Variant,
    Resource,
}

/// Build symbol index from AST
fn build_symbol_index(ast: &WitFile) -> Vec<SymbolDef> {
    let mut symbols = Vec::new();

    for item in &ast.items {
        match item {
            TopLevelItem::Interface(iface) => {
                let iface_name = iface.name.name.clone();
                symbols.push(SymbolDef {
                    name: iface_name.clone(),
                    kind: SymbolDefKind::Interface,
                    span: iface.span.clone(),
                    container: None,
                });

                for item in &iface.items {
                    match item {
                        InterfaceItem::Func(func) => {
                            let params: Vec<String> = func
                                .params
                                .iter()
                                .map(|p| format!("{}: {}", p.name.name, ty_to_string(&p.ty)))
                                .collect();
                            let result = func.result.as_ref().map(ty_to_string);

                            symbols.push(SymbolDef {
                                name: func.name.name.clone(),
                                kind: SymbolDefKind::Func { params, result },
                                span: func.span.clone(),
                                container: Some(iface_name.clone()),
                            });
                        }
                        InterfaceItem::TypeDef(typedef) => {
                            let (name, kind) = typedef_to_symbol(&typedef.kind);
                            symbols.push(SymbolDef {
                                name,
                                kind,
                                span: typedef.span.clone(),
                                container: Some(iface_name.clone()),
                            });
                        }
                        InterfaceItem::Use(_) => {}
                    }
                }
            }
            TopLevelItem::World(world) => {
                let world_name = world.name.name.clone();
                symbols.push(SymbolDef {
                    name: world_name.clone(),
                    kind: SymbolDefKind::World,
                    span: world.span.clone(),
                    container: None,
                });

                for item in &world.items {
                    if let WorldItem::TypeDef(typedef) = item {
                        let (name, kind) = typedef_to_symbol(&typedef.kind);
                        symbols.push(SymbolDef {
                            name,
                            kind,
                            span: typedef.span.clone(),
                            container: Some(world_name.clone()),
                        });
                    }
                }
            }
            TopLevelItem::Use(_) | TopLevelItem::NestedPackage(_) => {}
        }
    }

    symbols
}

fn typedef_to_symbol(kind: &TypeDefKind) -> (String, SymbolDefKind) {
    match kind {
        TypeDefKind::Record { name, fields, .. } => {
            let field_names: Vec<_> = fields.iter().map(|f| f.name.name.clone()).collect();
            (
                name.name.clone(),
                SymbolDefKind::Type {
                    description: format!("record {{ {} }}", field_names.join(", ")),
                },
            )
        }
        TypeDefKind::Enum { name, cases } => (
            name.name.clone(),
            SymbolDefKind::Type {
                description: format!(
                    "enum {{ {} }}",
                    cases
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            },
        ),
        TypeDefKind::Variant { name, cases, .. } => {
            let case_names: Vec<_> = cases.iter().map(|c| c.name.name.clone()).collect();
            (
                name.name.clone(),
                SymbolDefKind::Type {
                    description: format!("variant {{ {} }}", case_names.join(", ")),
                },
            )
        }
        TypeDefKind::Flags { name, flags } => (
            name.name.clone(),
            SymbolDefKind::Type {
                description: format!(
                    "flags {{ {} }}",
                    flags
                        .iter()
                        .map(|f| f.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            },
        ),
        TypeDefKind::Resource { name, .. } => (name.name.clone(), SymbolDefKind::Resource),
        TypeDefKind::Alias { name, ty, .. } => (
            name.name.clone(),
            SymbolDefKind::Type {
                description: format!("type = {}", ty_to_string(ty)),
            },
        ),
    }
}

fn ty_to_string(ty: &Ty) -> String {
    match ty {
        Ty::Primitive(p, _) => primitive_to_string(p).to_string(),
        Ty::Named(id) => id.name.clone(),
        Ty::List { element, .. } => format!("list<{}>", ty_to_string(element)),
        Ty::Option { inner, .. } => format!("option<{}>", ty_to_string(inner)),
        Ty::Result { ok, err, .. } => match (ok, err) {
            (Some(ok), Some(err)) => format!("result<{}, {}>", ty_to_string(ok), ty_to_string(err)),
            (Some(ok), None) => format!("result<{}>", ty_to_string(ok)),
            (None, Some(err)) => format!("result<_, {}>", ty_to_string(err)),
            (None, None) => "result".to_string(),
        },
        Ty::Tuple { elements, .. } => {
            format!(
                "tuple<{}>",
                elements
                    .iter()
                    .map(ty_to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        Ty::Future { inner, .. } => inner
            .as_ref()
            .map(|i| format!("future<{}>", ty_to_string(i)))
            .unwrap_or_else(|| "future".to_string()),
        Ty::Stream { inner, .. } => inner
            .as_ref()
            .map(|i| format!("stream<{}>", ty_to_string(i)))
            .unwrap_or_else(|| "stream".to_string()),
        Ty::Borrow { resource, .. } => format!("borrow<{}>", resource.name),
        Ty::Own { resource, .. } => format!("own<{}>", resource.name),
        Ty::Generic { name, args, .. } => {
            let args_str: Vec<_> = args.iter().map(ty_to_string).collect();
            format!("{}<{}>", name.name, args_str.join(", "))
        }
    }
}

fn primitive_to_string(p: &PrimitiveTy) -> &'static str {
    match p {
        PrimitiveTy::U8 => "u8",
        PrimitiveTy::U16 => "u16",
        PrimitiveTy::U32 => "u32",
        PrimitiveTy::U64 => "u64",
        PrimitiveTy::S8 => "s8",
        PrimitiveTy::S16 => "s16",
        PrimitiveTy::S32 => "s32",
        PrimitiveTy::S64 => "s64",
        PrimitiveTy::F32 => "f32",
        PrimitiveTy::F64 => "f64",
        PrimitiveTy::Bool => "bool",
        PrimitiveTy::Char => "char",
        PrimitiveTy::String => "string",
    }
}

// ============================================================================
// Comment region detection (for filtering false parse errors)
// ============================================================================

/// Find all comment byte ranges in source code.
///
/// Scans for `//...<newline>` and `/* ... */` comments, returning their byte
/// ranges. This is used to filter out spurious parse errors that tree-sitter
/// generates for content inside comments.
fn find_comment_ranges(source: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            // Line comment: // ... \n
            let start = i;
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            ranges.push(start..i);
        } else if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            // Block comment: /* ... */
            let start = i;
            i += 2;
            while i + 1 < len {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    break;
                }
                i += 1;
            }
            ranges.push(start..i);
        } else if bytes[i] == b'"' {
            // Skip string literals to avoid false positives from // inside strings
            i += 1;
            while i < len && bytes[i] != b'"' {
                if bytes[i] == b'\\' {
                    i += 1; // skip escaped char
                }
                i += 1;
            }
            if i < len {
                i += 1; // skip closing quote
            }
        } else {
            i += 1;
        }
    }

    ranges
}

/// Check if a byte offset falls inside any comment range.
fn is_in_comment(offset: usize, comment_ranges: &[std::ops::Range<usize>]) -> bool {
    comment_ranges
        .iter()
        .any(|r| offset >= r.start && offset < r.end)
}

/// Replace comment contents with spaces (preserving newlines) so line/column
/// structure stays stable while removing comment text from parsing.
fn strip_comments_preserve_layout(
    source: &str,
    comment_ranges: &[std::ops::Range<usize>],
) -> String {
    let mut bytes = source.as_bytes().to_vec();

    for range in comment_ranges {
        for byte in &mut bytes[range.start..range.end] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    }

    String::from_utf8(bytes).unwrap_or_else(|_| source.to_string())
}

/// Detect whether parse errors are purely comment-induced noise.
///
/// Heuristic: if parsing the same source with comments stripped yields no
/// parse errors, we treat original parse errors as comment noise.
fn has_only_comment_induced_parse_errors(source: &str) -> bool {
    let comment_ranges = find_comment_ranges(source);
    if comment_ranges.is_empty() {
        return false;
    }

    let stripped = strip_comments_preserve_layout(source, &comment_ranges);
    let (_, stripped_errors) = parse_file(&stripped);
    stripped_errors.is_empty()
}

/// Parse the source with comments stripped (layout preserved) and return the
/// AST only if the stripped parse is clean.
fn parse_clean_without_comments(source: &str) -> Option<WitFile> {
    let comment_ranges = find_comment_ranges(source);
    if comment_ranges.is_empty() {
        return None;
    }

    let stripped = strip_comments_preserve_layout(source, &comment_ranges);
    let (ast, errors) = parse_file(&stripped);
    if errors.is_empty() { ast } else { None }
}

// ============================================================================
// Document Symbols (hierarchical outline)
// ============================================================================

/// Build a hierarchical document symbol tree from a parsed AST.
///
/// Produces nested `DocumentSymbol` nodes suitable for VS Code's outline view:
/// - Interfaces → children: funcs, typedefs
/// - Worlds → children: imports, exports, typedefs
pub fn build_document_symbols(content: &str, ast: &WitFile) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();

    for item in &ast.items {
        match item {
            TopLevelItem::Interface(iface) => {
                let mut children = Vec::new();

                for iitem in &iface.items {
                    match iitem {
                        InterfaceItem::Func(func) => {
                            let params_str: Vec<String> = func
                                .params
                                .iter()
                                .map(|p| format!("{}: {}", p.name.name, ty_to_string(&p.ty)))
                                .collect();
                            let result_str = func
                                .result
                                .as_ref()
                                .map(|r| format!(" -> {}", ty_to_string(r)))
                                .unwrap_or_default();
                            let detail = format!("func({}){}", params_str.join(", "), result_str);

                            #[allow(deprecated)]
                            children.push(DocumentSymbol {
                                name: func.name.name.clone(),
                                detail: Some(detail),
                                kind: SymbolKind::FUNCTION,
                                tags: None,
                                deprecated: None,
                                range: span_to_range(content, func.span.clone()),
                                selection_range: span_to_range(content, func.name.span.clone()),
                                children: None,
                            });
                        }
                        InterfaceItem::TypeDef(typedef) => {
                            if let Some(sym) = typedef_to_document_symbol(content, typedef) {
                                children.push(sym);
                            }
                        }
                        InterfaceItem::Use(_) => {}
                    }
                }

                #[allow(deprecated)]
                symbols.push(DocumentSymbol {
                    name: iface.name.name.clone(),
                    detail: Some(format!("interface ({} items)", children.len())),
                    kind: SymbolKind::INTERFACE,
                    tags: None,
                    deprecated: None,
                    range: span_to_range(content, iface.span.clone()),
                    selection_range: span_to_range(content, iface.name.span.clone()),
                    children: if children.is_empty() {
                        None
                    } else {
                        Some(children)
                    },
                });
            }
            TopLevelItem::World(world) => {
                let mut children = Vec::new();

                for witem in &world.items {
                    match witem {
                        WorldItem::Import(ie) | WorldItem::Export(ie) => {
                            let direction = if matches!(witem, WorldItem::Import(_)) {
                                "import"
                            } else {
                                "export"
                            };
                            let name = ie
                                .name
                                .as_ref()
                                .map(|n| n.name.clone())
                                .unwrap_or_else(|| direction.to_string());

                            #[allow(deprecated)]
                            children.push(DocumentSymbol {
                                name,
                                detail: Some(direction.to_string()),
                                kind: SymbolKind::PROPERTY,
                                tags: None,
                                deprecated: None,
                                range: span_to_range(content, ie.span.clone()),
                                selection_range: span_to_range(
                                    content,
                                    ie.name
                                        .as_ref()
                                        .map(|n| n.span.clone())
                                        .unwrap_or_else(|| ie.span.start..ie.span.start),
                                ),
                                children: None,
                            });
                        }
                        WorldItem::TypeDef(typedef) => {
                            if let Some(sym) = typedef_to_document_symbol(content, typedef) {
                                children.push(sym);
                            }
                        }
                        WorldItem::Use(_) | WorldItem::Include(_) => {}
                    }
                }

                #[allow(deprecated)]
                symbols.push(DocumentSymbol {
                    name: world.name.name.clone(),
                    detail: Some(format!("world ({} items)", children.len())),
                    kind: SymbolKind::MODULE,
                    tags: None,
                    deprecated: None,
                    range: span_to_range(content, world.span.clone()),
                    selection_range: span_to_range(content, world.name.span.clone()),
                    children: if children.is_empty() {
                        None
                    } else {
                        Some(children)
                    },
                });
            }
            TopLevelItem::Use(_) | TopLevelItem::NestedPackage(_) => {}
        }
    }

    // Safety: clamp all selection_ranges to be within their range.
    for sym in &mut symbols {
        clamp_selection_range(sym);
    }

    symbols
}

/// Convert a TypeDef AST node to a DocumentSymbol.
fn typedef_to_document_symbol(
    content: &str,
    typedef: &kettu_parser::TypeDef,
) -> Option<DocumentSymbol> {
    let (name, detail, kind) = match &typedef.kind {
        TypeDefKind::Record { name, fields, .. } => {
            let field_names: Vec<_> = fields.iter().map(|f| f.name.name.as_str()).collect();
            (
                name,
                format!("record {{ {} }}", field_names.join(", ")),
                SymbolKind::STRUCT,
            )
        }
        TypeDefKind::Enum { name, cases } => {
            let case_names: Vec<_> = cases.iter().map(|c| c.name.as_str()).collect();
            (
                name,
                format!("enum {{ {} }}", case_names.join(", ")),
                SymbolKind::ENUM,
            )
        }
        TypeDefKind::Variant { name, cases, .. } => {
            let case_names: Vec<_> = cases.iter().map(|c| c.name.name.as_str()).collect();
            (
                name,
                format!("variant {{ {} }}", case_names.join(", ")),
                SymbolKind::ENUM,
            )
        }
        TypeDefKind::Flags { name, flags } => {
            let flag_names: Vec<_> = flags.iter().map(|f| f.name.as_str()).collect();
            (
                name,
                format!("flags {{ {} }}", flag_names.join(", ")),
                SymbolKind::ENUM,
            )
        }
        TypeDefKind::Resource { name, .. } => (name, "resource".to_string(), SymbolKind::CLASS),
        TypeDefKind::Alias { name, ty, .. } => (
            name,
            format!("type = {}", ty_to_string(ty)),
            SymbolKind::TYPE_PARAMETER,
        ),
    };

    #[allow(deprecated)]
    Some(DocumentSymbol {
        name: name.name.clone(),
        detail: Some(detail),
        kind,
        tags: None,
        deprecated: None,
        range: span_to_range(content, typedef.span.clone()),
        selection_range: span_to_range(content, name.span.clone()),
        children: None,
    })
}

/// Recursively ensure every DocumentSymbol's `selection_range` is contained
/// within its `range`, as required by the LSP protocol.
fn clamp_selection_range(sym: &mut DocumentSymbol) {
    let r = &sym.range;
    let s = &mut sym.selection_range;

    // Clamp start
    if s.start.line < r.start.line
        || (s.start.line == r.start.line && s.start.character < r.start.character)
    {
        s.start = r.start;
    }
    // Clamp end
    if s.end.line > r.end.line || (s.end.line == r.end.line && s.end.character > r.end.character) {
        s.end = r.end;
    }
    // Ensure start <= end after clamping
    if s.start.line > s.end.line
        || (s.start.line == s.end.line && s.start.character > s.end.character)
    {
        s.end = s.start;
    }

    if let Some(children) = &mut sym.children {
        for child in children {
            clamp_selection_range(child);
        }
    }
}

// ============================================================================
// LSP Backend
// ============================================================================

/// LSP Backend
pub struct Backend {
    client: Client,
    documents: RwLock<HashMap<Url, Document>>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: RwLock::new(HashMap::new()),
        }
    }

    async fn publish_diagnostics(&self, uri: Url, content: &str, version: i32) {
        let (ast, parse_errors) = parse_file(content);

        // Find comment byte ranges so we can filter out false parse errors
        // from inside comments (tree-sitter generates ERROR nodes for comment content)
        let comment_ranges = find_comment_ranges(content);

        let suppress_comment_noise = ast.is_some()
            && !parse_errors.is_empty()
            && has_only_comment_induced_parse_errors(content);

        let stripped_ast_for_check = if suppress_comment_noise {
            parse_clean_without_comments(content)
        } else {
            None
        };

        let ast_for_check = stripped_ast_for_check.as_ref().or(ast.as_ref());

        let mut diagnostics: Vec<tower_lsp::lsp_types::Diagnostic> = parse_errors
            .into_iter()
            .filter(|_| !suppress_comment_noise)
            .filter(|e| !is_in_comment(e.error_position.bytes.start, &comment_ranges))
            .map(|e| {
                let range = span_to_range(content, e.error_position.bytes.clone());
                tower_lsp::lsp_types::Diagnostic {
                    range,
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("kettu-parser".to_string()),
                    message: e.to_string(),
                    ..Default::default()
                }
            })
            .collect();

        // Type checking
        if let Some(ast) = ast_for_check {
            let check_errors = check(ast);
            for err in check_errors {
                let range = span_to_range(content, err.span.clone());
                diagnostics.push(tower_lsp::lsp_types::Diagnostic {
                    range,
                    severity: Some(match err.severity {
                        Severity::Error => DiagnosticSeverity::ERROR,
                        Severity::Warning => DiagnosticSeverity::WARNING,
                        Severity::Info => DiagnosticSeverity::INFORMATION,
                    }),
                    source: Some("kettu-checker".to_string()),
                    message: err.message,
                    ..Default::default()
                });
            }
        }

        self.client
            .publish_diagnostics(uri, diagnostics, Some(version))
            .await;
    }

    /// Find symbol at a given position
    fn find_symbol_at_position(&self, doc: &Document, position: Position) -> Option<SymbolDef> {
        let ast = doc.ast.as_ref()?;
        let offset = position_to_offset(&doc.content, position)?;
        let symbols = build_symbol_index(ast);

        // Find the smallest symbol that contains this position
        symbols
            .into_iter()
            .filter(|s| s.span.contains(&offset))
            .min_by_key(|s| s.span.len())
    }
}

fn is_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '%'
}

/// Extract identifier text under (or immediately left of) the cursor.
fn identifier_at_position(content: &str, position: Position) -> Option<String> {
    let line = content.lines().nth(position.line as usize)?;
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }

    let mut idx = position.character as usize;
    if idx >= chars.len() {
        idx = chars.len().saturating_sub(1);
    }

    if !is_ident_char(chars[idx]) {
        if idx == 0 || !is_ident_char(chars[idx - 1]) {
            return None;
        }
        idx -= 1;
    }

    let mut start = idx;
    while start > 0 && is_ident_char(chars[start - 1]) {
        start -= 1;
    }

    let mut end = idx;
    while end + 1 < chars.len() && is_ident_char(chars[end + 1]) {
        end += 1;
    }

    Some(chars[start..=end].iter().collect())
}

/// Resolve a definition range at a given cursor position.
///
/// Priority:
/// 1. Identifier-based lookup by symbol name (for references like `export cli;`)
/// 2. Fallback to containment lookup (for clicks inside declarations)
fn find_definition_range(content: &str, ast: &WitFile, position: Position) -> Option<Range> {
    let symbols = build_symbol_index(ast);

    if let Some(ident) = identifier_at_position(content, position) {
        if let Some(symbol) = symbols
            .iter()
            .filter(|s| s.name == ident)
            .min_by_key(|s| s.span.len())
        {
            return Some(span_to_range(content, symbol.span.clone()));
        }
    }

    let offset = position_to_offset(content, position)?;
    symbols
        .into_iter()
        .filter(|s| s.span.contains(&offset))
        .min_by_key(|s| s.span.len())
        .map(|s| span_to_range(content, s.span))
}

fn get_qualified_reference_at_position(
    content: &str,
    position: Position,
) -> Option<(String, String)> {
    let line = content.lines().nth(position.line as usize)?;
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }

    let mut idx = position.character as usize;
    if idx >= chars.len() {
        idx = chars.len().saturating_sub(1);
    }

    if !is_ident_char(chars[idx]) {
        if idx == 0 || !is_ident_char(chars[idx - 1]) {
            return None;
        }
        idx -= 1;
    }

    let mut start = idx;
    while start > 0 && is_ident_char(chars[start - 1]) {
        start -= 1;
    }

    let mut end = idx;
    while end + 1 < chars.len() && is_ident_char(chars[end + 1]) {
        end += 1;
    }

    let skip_ws_left = |mut i: usize| -> Option<usize> {
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        if i == 0 { None } else { Some(i - 1) }
    };
    let skip_ws_right = |mut i: usize| -> Option<usize> {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i < chars.len() { Some(i) } else { None }
    };

    let dot_left = skip_ws_left(start);
    if let Some(dot_idx) = dot_left.filter(|d| chars[*d] == '.') {
        let mut q_end = dot_idx;
        while q_end > 0 && chars[q_end - 1].is_whitespace() {
            q_end -= 1;
        }
        if q_end > 0 {
            let mut q_start = q_end;
            while q_start > 0 && is_ident_char(chars[q_start - 1]) {
                q_start -= 1;
            }
            if q_start < q_end {
                let qualifier: String = chars[q_start..q_end].iter().collect();
                let member: String = chars[start..=end].iter().collect();
                if !qualifier.is_empty() && !member.is_empty() {
                    return Some((qualifier, member));
                }
            }
        }
    }

    let dot_right = skip_ws_right(end + 1);
    if let Some(dot_idx) = dot_right.filter(|d| chars[*d] == '.') {
        let mut m_start = dot_idx + 1;
        while m_start < chars.len() && chars[m_start].is_whitespace() {
            m_start += 1;
        }
        if m_start < chars.len() && is_ident_char(chars[m_start]) {
            let mut m_end = m_start;
            while m_end + 1 < chars.len() && is_ident_char(chars[m_end + 1]) {
                m_end += 1;
            }
            let qualifier: String = chars[start..=end].iter().collect();
            let member: String = chars[m_start..=m_end].iter().collect();
            if !qualifier.is_empty() && !member.is_empty() {
                return Some((qualifier, member));
            }
        }
    }

    None
}

fn find_interface_or_member_range(
    content: &str,
    ast: &WitFile,
    interface_name: &str,
    member_name: Option<&str>,
) -> Option<Range> {
    let symbols = build_symbol_index(ast);

    if let Some(member_name) = member_name {
        return symbols
            .iter()
            .filter(|s| s.name == member_name)
            .filter(|s| s.container.as_deref() == Some(interface_name))
            .min_by_key(|s| s.span.len())
            .map(|s| span_to_range(content, s.span.clone()));
    }

    symbols
        .iter()
        .filter(|s| s.name == interface_name)
        .find(|s| matches!(s.kind, SymbolDefKind::Interface))
        .map(|s| span_to_range(content, s.span.clone()))
}

fn find_interface_or_member_symbol(
    ast: &WitFile,
    interface_name: &str,
    member_name: Option<&str>,
) -> Option<SymbolDef> {
    let symbols = build_symbol_index(ast);

    if let Some(member_name) = member_name {
        return symbols
            .into_iter()
            .filter(|s| s.name == member_name)
            .filter(|s| s.container.as_deref() == Some(interface_name))
            .min_by_key(|s| s.span.len());
    }

    symbols
        .into_iter()
        .filter(|s| s.name == interface_name)
        .find(|s| matches!(s.kind, SymbolDefKind::Interface))
}

fn find_imported_symbol_for_hover(
    source_uri: &Url,
    source_doc: &Document,
    position: Position,
    docs: &HashMap<Url, Document>,
) -> Option<SymbolDef> {
    let source_path = source_uri.to_file_path().ok()?;
    let source_ast = source_doc.ast.as_ref()?;
    let resolved = resolve_imports(&source_path, source_ast);

    let ident = identifier_at_position(&source_doc.content, position)?;
    let qualified = get_qualified_reference_at_position(&source_doc.content, position);

    let (import_key, member_name) = match qualified {
        Some((qualifier, member)) => {
            if ident == qualifier {
                (qualifier, None)
            } else {
                (qualifier, Some(member))
            }
        }
        None => (ident, None),
    };

    let (target_path, interface_name) = resolved.imports.get(&import_key)?.clone();
    let target_uri = Url::from_file_path(&target_path).ok()?;

    if let Some(target_doc) = docs.get(&target_uri) {
        let target_ast = target_doc.ast.as_ref()?;
        return find_interface_or_member_symbol(
            target_ast,
            &interface_name,
            member_name.as_deref(),
        );
    }

    let content = std::fs::read_to_string(Path::new(&target_path)).ok()?;
    let parsed_doc = Document::new(content, 0);
    let target_ast = parsed_doc.ast.as_ref()?;
    find_interface_or_member_symbol(target_ast, &interface_name, member_name.as_deref())
}

fn symbol_markdown(symbol: &SymbolDef) -> String {
    match &symbol.kind {
        SymbolDefKind::Interface => format!("**interface** `{}`", symbol.name),
        SymbolDefKind::World => format!("**world** `{}`", symbol.name),
        SymbolDefKind::Func { params, result } => {
            let params_str = params.join(", ");
            let result_str = result
                .as_ref()
                .map(|r| format!(" -> {}", r))
                .unwrap_or_default();
            format!("**func** `{}`({}){}", symbol.name, params_str, result_str)
        }
        SymbolDefKind::Type { description } => {
            format!("**type** `{}`: {}", symbol.name, description)
        }
        SymbolDefKind::Record => format!("**record** `{}`", symbol.name),
        SymbolDefKind::Enum => format!("**enum** `{}`", symbol.name),
        SymbolDefKind::Variant => format!("**variant** `{}`", symbol.name),
        SymbolDefKind::Resource => format!("**resource** `{}`", symbol.name),
    }
}

fn find_named_symbol_for_hover(ast: &WitFile, ident: &str) -> Option<SymbolDef> {
    build_symbol_index(ast)
        .into_iter()
        .filter(|s| s.name == ident)
        .min_by_key(|s| s.span.len())
}

fn find_param_hover(
    ast: &WitFile,
    ident: &str,
    offset: usize,
) -> Option<(String, std::ops::Range<usize>)> {
    for item in &ast.items {
        match item {
            TopLevelItem::Interface(iface) => {
                for iface_item in &iface.items {
                    if let InterfaceItem::Func(func) = iface_item {
                        if func.span.contains(&offset) {
                            for param in &func.params {
                                if param.name.name == ident {
                                    let markdown = format!(
                                        "**param** `{}`: {}",
                                        param.name.name,
                                        ty_to_string(&param.ty)
                                    );
                                    return Some((markdown, param.name.span.clone()));
                                }
                            }
                        }
                    }
                }
            }
            TopLevelItem::World(world) => {
                for world_item in &world.items {
                    match world_item {
                        WorldItem::Import(ie) | WorldItem::Export(ie) => {
                            if let ImportExportKind::Func(func) = &ie.kind {
                                if func.span.contains(&offset) {
                                    for param in &func.params {
                                        if param.name.name == ident {
                                            let markdown = format!(
                                                "**param** `{}`: {}",
                                                param.name.name,
                                                ty_to_string(&param.ty)
                                            );
                                            return Some((markdown, param.name.span.clone()));
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    None
}

#[cfg(test)]
fn find_local_hover_info(
    content: &str,
    ast: &WitFile,
    position: Position,
) -> Option<(String, Option<Range>)> {
    let inference_ctx = build_inference_context_for_hover(None, None, None, ast);
    find_local_hover_info_with_call_returns(content, ast, position, &inference_ctx)
}

#[derive(Default)]
struct InferenceContext {
    call_returns: HashMap<String, String>,
    record_fields: HashMap<String, HashMap<String, String>>,
    variant_cases: HashMap<String, HashMap<String, Option<String>>>,
}

fn find_local_hover_info_with_call_returns(
    content: &str,
    ast: &WitFile,
    position: Position,
    inference_ctx: &InferenceContext,
) -> Option<(String, Option<Range>)> {
    if let Some(ident) = identifier_at_position(content, position) {
        let offset = position_to_offset(content, position)?;

        if let Some((markdown, span)) = find_param_hover(ast, &ident, offset) {
            return Some((markdown, Some(span_to_range(content, span))));
        }

        if let Some(symbol) = find_named_symbol_for_hover(ast, &ident) {
            return Some((
                symbol_markdown(&symbol),
                Some(span_to_range(content, symbol.span)),
            ));
        }

        if let Some(ty) = infer_local_let_type_for_ident(ast, &ident, offset, inference_ctx) {
            return Some((format!("**let** `{}`: {}", ident, ty), None));
        }

        // Don't fall back to container-level hover for identifier tokens with no
        // matching symbol; this avoids misleading function-level hover in expressions.
        return None;
    }

    let offset = position_to_offset(content, position)?;
    let symbol = build_symbol_index(ast)
        .into_iter()
        .filter(|s| s.span.contains(&offset))
        .min_by_key(|s| s.span.len())?;

    Some((
        symbol_markdown(&symbol),
        Some(span_to_range(content, symbol.span)),
    ))
}

fn build_inference_context_for_hover(
    source_uri: Option<&Url>,
    source_doc: Option<&Document>,
    docs: Option<&HashMap<Url, Document>>,
    ast: &WitFile,
) -> InferenceContext {
    let mut ctx = InferenceContext::default();

    // Local callable returns by unqualified name.
    for item in &ast.items {
        match item {
            TopLevelItem::Interface(iface) => {
                for iface_item in &iface.items {
                    if let InterfaceItem::Func(func) = iface_item {
                        if let Some(result) = &func.result {
                            ctx.call_returns
                                .insert(func.name.name.clone(), ty_to_string(result));
                        }
                    }

                    if let InterfaceItem::TypeDef(typedef) = iface_item {
                        if let TypeDefKind::Record { name, fields, .. } = &typedef.kind {
                            let mut field_map = HashMap::new();
                            for f in fields {
                                field_map.insert(f.name.name.clone(), ty_to_string(&f.ty));
                            }
                            ctx.record_fields.insert(name.name.clone(), field_map);
                        }

                        if let TypeDefKind::Variant { name, cases, .. } = &typedef.kind {
                            let mut case_map = HashMap::new();
                            for c in cases {
                                case_map
                                    .insert(c.name.name.clone(), c.ty.as_ref().map(ty_to_string));
                            }
                            ctx.variant_cases.insert(name.name.clone(), case_map);
                        }
                    }
                }
            }
            TopLevelItem::World(world) => {
                for world_item in &world.items {
                    if let WorldItem::Import(ie) | WorldItem::Export(ie) = world_item {
                        if let ImportExportKind::Func(func) = &ie.kind {
                            if let Some(result) = &func.result {
                                let ty = ty_to_string(result);
                                ctx.call_returns.insert(func.name.name.clone(), ty.clone());
                                if let Some(name) = &ie.name {
                                    ctx.call_returns.entry(name.name.clone()).or_insert(ty);
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Imported callable returns by qualified name (alias.member).
    if let (Some(source_uri), Some(source_doc), Some(docs)) = (source_uri, source_doc, docs) {
        if let (Ok(source_path), Some(source_ast)) =
            (source_uri.to_file_path(), source_doc.ast.as_ref())
        {
            let resolved = resolve_imports(&source_path, source_ast);
            for (alias, (target_path, interface_name)) in resolved.imports {
                let target_ast = if let Ok(target_uri) = Url::from_file_path(&target_path) {
                    docs.get(&target_uri).and_then(|d| d.ast.as_ref()).cloned()
                } else {
                    None
                };

                let target_ast = match target_ast {
                    Some(ast) => ast,
                    None => {
                        let Ok(content) = std::fs::read_to_string(Path::new(&target_path)) else {
                            continue;
                        };
                        let parsed = Document::new(content, 0);
                        let Some(ast) = parsed.ast else {
                            continue;
                        };
                        ast
                    }
                };

                for item in &target_ast.items {
                    if let TopLevelItem::Interface(iface) = item {
                        if iface.name.name != interface_name {
                            continue;
                        }

                        for iface_item in &iface.items {
                            if let InterfaceItem::Func(func) = iface_item {
                                if let Some(result) = &func.result {
                                    ctx.call_returns.insert(
                                        format!("{}.{}", alias, func.name.name),
                                        ty_to_string(result),
                                    );
                                }
                            }

                            if let InterfaceItem::TypeDef(typedef) = iface_item {
                                if let TypeDefKind::Record { name, fields, .. } = &typedef.kind {
                                    let mut field_map = HashMap::new();
                                    for f in fields {
                                        field_map.insert(f.name.name.clone(), ty_to_string(&f.ty));
                                    }
                                    ctx.record_fields
                                        .entry(name.name.clone())
                                        .or_insert(field_map);
                                }

                                if let TypeDefKind::Variant { name, cases, .. } = &typedef.kind {
                                    let mut case_map = HashMap::new();
                                    for c in cases {
                                        case_map.insert(
                                            c.name.name.clone(),
                                            c.ty.as_ref().map(ty_to_string),
                                        );
                                    }
                                    ctx.variant_cases
                                        .entry(name.name.clone())
                                        .or_insert(case_map);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    ctx
}

fn infer_local_let_type_for_ident(
    ast: &WitFile,
    ident: &str,
    offset: usize,
    inference_ctx: &InferenceContext,
) -> Option<String> {
    for item in &ast.items {
        match item {
            TopLevelItem::Interface(iface) => {
                for iface_item in &iface.items {
                    if let InterfaceItem::Func(func) = iface_item {
                        if !func.span.contains(&offset) {
                            continue;
                        }

                        let Some(body) = func.body.as_ref() else {
                            continue;
                        };
                        let mut scope: HashMap<String, String> = HashMap::new();
                        for p in &func.params {
                            scope.insert(p.name.name.clone(), ty_to_string(&p.ty));
                        }
                        collect_let_bindings_before(
                            &body.statements,
                            offset,
                            &mut scope,
                            inference_ctx,
                        );
                        return scope.get(ident).cloned();
                    }
                }
            }
            TopLevelItem::World(world) => {
                for world_item in &world.items {
                    match world_item {
                        WorldItem::Import(ie) | WorldItem::Export(ie) => {
                            if let ImportExportKind::Func(func) = &ie.kind {
                                if !func.span.contains(&offset) {
                                    continue;
                                }

                                let Some(body) = func.body.as_ref() else {
                                    continue;
                                };
                                let mut scope: HashMap<String, String> = HashMap::new();
                                for p in &func.params {
                                    scope.insert(p.name.name.clone(), ty_to_string(&p.ty));
                                }
                                collect_let_bindings_before(
                                    &body.statements,
                                    offset,
                                    &mut scope,
                                    inference_ctx,
                                );
                                return scope.get(ident).cloned();
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    None
}

fn collect_let_bindings_before(
    statements: &[Statement],
    before_offset: usize,
    scope: &mut HashMap<String, String>,
    inference_ctx: &InferenceContext,
) {
    for stmt in statements {
        match stmt {
            Statement::Let { name, value } => {
                if name.span.start < before_offset {
                    if let Some(ty) = infer_expr_type(value, scope, inference_ctx) {
                        scope.insert(name.name.clone(), ty);
                    }
                }
            }
            Statement::Expr(expr) => {
                collect_let_bindings_in_expr(expr, before_offset, scope, inference_ctx)
            }
            Statement::Return(Some(expr)) => {
                collect_let_bindings_in_expr(expr, before_offset, scope, inference_ctx)
            }
            Statement::Assign { .. }
            | Statement::Return(None)
            | Statement::Break { .. }
            | Statement::Continue { .. } => {}
        }
    }
}

fn collect_let_bindings_in_expr(
    expr: &Expr,
    before_offset: usize,
    scope: &mut HashMap<String, String>,
    inference_ctx: &InferenceContext,
) {
    match expr {
        Expr::If {
            then_branch,
            else_branch,
            ..
        } => {
            collect_let_bindings_before(then_branch, before_offset, scope, inference_ctx);
            if let Some(else_branch) = else_branch {
                collect_let_bindings_before(else_branch, before_offset, scope, inference_ctx);
            }
        }
        Expr::While { body, .. } | Expr::For { body, .. } | Expr::ForEach { body, .. } => {
            collect_let_bindings_before(body, before_offset, scope, inference_ctx);
        }
        Expr::Match { arms, .. } => {
            for arm in arms {
                collect_let_bindings_before(&arm.body, before_offset, scope, inference_ctx);
            }
        }
        _ => {}
    }
}

fn infer_expr_type(
    expr: &Expr,
    scope: &HashMap<String, String>,
    inference_ctx: &InferenceContext,
) -> Option<String> {
    match expr {
        Expr::Integer(_, _) => Some("s32".to_string()),
        Expr::String(_, _) | Expr::InterpolatedString(_, _) => Some("string".to_string()),
        Expr::Bool(_, _) => Some("bool".to_string()),
        Expr::Ident(id) => scope.get(&id.name).cloned(),
        Expr::Call { func, .. } => match func.as_ref() {
            Expr::Ident(id) => inference_ctx.call_returns.get(&id.name).cloned(),
            Expr::Field { expr, field, .. } => {
                if let Expr::Ident(qualifier) = expr.as_ref() {
                    inference_ctx
                        .call_returns
                        .get(&format!("{}.{}", qualifier.name, field.name))
                        .cloned()
                } else {
                    None
                }
            }
            _ => None,
        },
        Expr::Field { expr, field, .. } => {
            let owner_ty = infer_expr_type(expr, scope, inference_ctx)?;
            inference_ctx
                .record_fields
                .get(&owner_ty)
                .and_then(|fields| fields.get(&field.name).cloned())
        }
        Expr::OptionalChain { expr, field, .. } => {
            let owner_opt_ty = infer_expr_type(expr, scope, inference_ctx)?;
            let owner_ty = parse_option_payload_type(&owner_opt_ty)?;
            let field_ty = inference_ctx
                .record_fields
                .get(&owner_ty)
                .and_then(|fields| fields.get(&field.name).cloned())?;
            Some(format!("option<{}>", field_ty))
        }
        Expr::Try { expr, .. } => {
            let source_ty = infer_expr_type(expr, scope, inference_ctx)?;
            if let Some(inner) = parse_option_payload_type(&source_ty) {
                return Some(inner);
            }
            if let Some((ok_ty, _)) = parse_result_payload_types(&source_ty) {
                return ok_ty;
            }
            None
        }
        Expr::Await { expr, .. } => {
            let source_ty = infer_expr_type(expr, scope, inference_ctx)?;
            parse_future_payload_type(&source_ty)
        }
        Expr::If {
            then_branch,
            else_branch,
            ..
        } => {
            let then_ty = infer_block_tail_type(then_branch, scope, inference_ctx);
            let else_ty = else_branch
                .as_ref()
                .and_then(|b| infer_block_tail_type(b, scope, inference_ctx));
            match (then_ty, else_ty) {
                (Some(t), Some(e)) if t == e => Some(t),
                _ => None,
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            let scrutinee_ty = infer_expr_type(scrutinee, scope, inference_ctx);
            infer_match_arms_type(arms, scope, inference_ctx, scrutinee_ty.as_deref())
        }
        Expr::Binary { lhs, op, rhs, .. } => match op {
            BinOp::Eq
            | BinOp::Ne
            | BinOp::Lt
            | BinOp::Le
            | BinOp::Gt
            | BinOp::Ge
            | BinOp::And
            | BinOp::Or => Some("bool".to_string()),
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                let lhs_ty = infer_expr_type(lhs, scope, inference_ctx);
                let rhs_ty = infer_expr_type(rhs, scope, inference_ctx);
                match (lhs_ty, rhs_ty) {
                    (Some(l), Some(r)) if l == r => Some(l),
                    (Some(l), None) => Some(l),
                    (None, Some(r)) => Some(r),
                    _ => None,
                }
            }
        },
        Expr::RecordLiteral {
            type_name: Some(type_name),
            ..
        } => Some(type_name.name.clone()),
        Expr::VariantLiteral {
            type_name: Some(type_name),
            case_name,
            payload,
            ..
        } => {
            let case_payload =
                infer_variant_case_payload(&type_name.name, &case_name.name, inference_ctx)?;
            let expects_payload = case_payload.is_some();
            let has_payload = payload.is_some();

            if expects_payload != has_payload {
                return None;
            }

            Some(type_name.name.clone())
        }
        _ => None,
    }
}

fn infer_block_tail_type(
    statements: &[Statement],
    scope: &HashMap<String, String>,
    inference_ctx: &InferenceContext,
) -> Option<String> {
    let last = statements.last()?;
    match last {
        Statement::Expr(expr) => infer_expr_type(expr, scope, inference_ctx),
        Statement::Return(Some(expr)) => infer_expr_type(expr, scope, inference_ctx),
        _ => None,
    }
}

fn infer_match_arms_type(
    arms: &[kettu_parser::MatchArm],
    scope: &HashMap<String, String>,
    inference_ctx: &InferenceContext,
    scrutinee_ty: Option<&str>,
) -> Option<String> {
    let mut inferred: Option<String> = None;

    for arm in arms {
        let mut arm_scope = scope.clone();
        add_pattern_binding_types(&arm.pattern, &mut arm_scope, inference_ctx, scrutinee_ty);
        let arm_ty = infer_block_tail_type(&arm.body, &arm_scope, inference_ctx)?;
        match &inferred {
            Some(existing) if existing != &arm_ty => return None,
            None => inferred = Some(arm_ty),
            _ => {}
        }
    }

    inferred
}

fn add_pattern_binding_types(
    pattern: &Pattern,
    scope: &mut HashMap<String, String>,
    inference_ctx: &InferenceContext,
    scrutinee_ty: Option<&str>,
) {
    let Pattern::Variant {
        type_name,
        case_name,
        binding,
        ..
    } = pattern
    else {
        return;
    };

    let Some(binding) = binding else {
        return;
    };

    let ty_source = type_name.as_ref().map(|t| t.name.as_str()).or(scrutinee_ty);

    let Some(payload_ty) =
        ty_source.and_then(|ty| infer_variant_payload_type(ty, &case_name.name, inference_ctx))
    else {
        return;
    };

    scope.insert(binding.name.clone(), payload_ty);
}

fn infer_variant_payload_type(
    ty_name: &str,
    case_name: &str,
    inference_ctx: &InferenceContext,
) -> Option<String> {
    infer_variant_case_payload(ty_name, case_name, inference_ctx).flatten()
}

fn infer_variant_case_payload(
    ty_name: &str,
    case_name: &str,
    inference_ctx: &InferenceContext,
) -> Option<Option<String>> {
    if let Some(inner) = parse_option_payload_type(ty_name) {
        return match case_name {
            "some" => Some(Some(inner)),
            "none" => Some(None),
            _ => None,
        };
    }

    if let Some((ok_ty, err_ty)) = parse_result_payload_types(ty_name) {
        return match case_name {
            "ok" => Some(ok_ty),
            "err" => Some(err_ty),
            _ => None,
        };
    }

    inference_ctx
        .variant_cases
        .get(ty_name)
        .and_then(|cases| cases.get(case_name).cloned())
}

fn parse_option_payload_type(ty_name: &str) -> Option<String> {
    let inner = ty_name.strip_prefix("option<")?.strip_suffix('>')?;
    Some(inner.trim().to_string())
}

fn parse_result_payload_types(ty_name: &str) -> Option<(Option<String>, Option<String>)> {
    let inner = ty_name.strip_prefix("result<")?.strip_suffix('>')?;
    let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();

    match parts.as_slice() {
        [ok] => Some((parse_result_side(ok), None)),
        [ok, err] => Some((parse_result_side(ok), parse_result_side(err))),
        _ => None,
    }
}

fn parse_result_side(side: &str) -> Option<String> {
    if side == "_" || side.is_empty() {
        None
    } else {
        Some(side.to_string())
    }
}

fn parse_future_payload_type(ty_name: &str) -> Option<String> {
    let inner = ty_name.strip_prefix("future<")?.strip_suffix('>')?;
    Some(inner.trim().to_string())
}

fn find_imported_definition(
    source_uri: &Url,
    source_doc: &Document,
    position: Position,
    docs: &HashMap<Url, Document>,
) -> Option<Location> {
    let source_path = source_uri.to_file_path().ok()?;
    let source_ast = source_doc.ast.as_ref()?;
    let resolved = resolve_imports(&source_path, source_ast);

    let ident = identifier_at_position(&source_doc.content, position)?;
    let qualified = get_qualified_reference_at_position(&source_doc.content, position);

    let (import_key, member_name) = match qualified {
        Some((qualifier, member)) => {
            if ident == qualifier {
                (qualifier, None)
            } else {
                (qualifier, Some(member))
            }
        }
        None => (ident, None),
    };

    let (target_path, interface_name) = resolved.imports.get(&import_key)?.clone();
    let target_uri = Url::from_file_path(&target_path).ok()?;

    if let Some(target_doc) = docs.get(&target_uri) {
        let target_ast = target_doc.ast.as_ref()?;
        let range = find_interface_or_member_range(
            &target_doc.content,
            target_ast,
            &interface_name,
            member_name.as_deref(),
        )?;
        return Some(Location {
            uri: target_uri,
            range,
        });
    }

    let content = std::fs::read_to_string(Path::new(&target_path)).ok()?;
    let parsed_doc = Document::new(content, 0);
    let target_ast = parsed_doc.ast.as_ref()?;
    let range = find_interface_or_member_range(
        &parsed_doc.content,
        target_ast,
        &interface_name,
        member_name.as_deref(),
    )?;

    Some(Location {
        uri: target_uri,
        range,
    })
}

fn span_to_range(content: &str, span: std::ops::Range<usize>) -> Range {
    let start = offset_to_position(content, span.start);
    let end = offset_to_position(content, span.end);
    Range { start, end }
}

fn offset_to_position(content: &str, offset: usize) -> Position {
    let mut line = 0;
    let mut col = 0;
    for (i, c) in content.chars().enumerate() {
        if i == offset {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    Position::new(line, col)
}

fn position_to_offset(content: &str, position: Position) -> Option<usize> {
    let mut line = 0;
    let mut col = 0;
    for (i, c) in content.chars().enumerate() {
        if line == position.line && col == position.character {
            return Some(i);
        }
        if c == '\n' {
            if line == position.line {
                return Some(i); // End of line
            }
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    // Handle end of file
    if line == position.line {
        Some(content.len())
    } else {
        None
    }
}

fn diagnostic_text_in_range<'a>(content: &'a str, range: &Range) -> Option<&'a str> {
    let start = position_to_offset(content, range.start)?;
    let end = position_to_offset(content, range.end)?;
    if start > end || end > content.len() {
        return None;
    }
    content.get(start..end)
}

fn remove_trailing_paren_group(text: &str) -> Option<String> {
    let trimmed = text.trim_end();
    if !trimmed.ends_with(')') {
        return None;
    }
    let open = trimmed.rfind('(')?;
    let prefix = &trimmed[..open];
    Some(prefix.to_string())
}

fn build_variant_arity_replacement(message: &str, original: &str) -> Option<(String, String)> {
    if message.contains("pattern requires a binding for payload") {
        if original.contains('(') {
            return None;
        }
        return Some((
            "Add payload binding to pattern".to_string(),
            format!("{}(value)", original.trim_end()),
        ));
    }

    if message.contains("pattern must not bind a payload") {
        let rewritten = remove_trailing_paren_group(original)?;
        return Some(("Remove payload binding from pattern".to_string(), rewritten));
    }

    if message.contains("requires a payload") {
        let trimmed = original.trim_end();
        if trimmed.ends_with("()") {
            let base = &trimmed[..trimmed.len() - 2];
            return Some((
                "Add payload argument".to_string(),
                format!("{}(/* payload */)", base),
            ));
        }
        if !trimmed.contains('(') {
            return Some((
                "Add payload argument".to_string(),
                format!("{}(/* payload */)", trimmed),
            ));
        }
        return None;
    }

    if message.contains("does not accept a payload") {
        let rewritten = remove_trailing_paren_group(original)?;
        return Some(("Remove payload argument".to_string(), rewritten));
    }

    None
}

fn variant_arity_code_actions(
    uri: &Url,
    content: &str,
    diagnostics: &[Diagnostic],
) -> Vec<CodeActionOrCommand> {
    let mut actions = Vec::new();

    for diagnostic in diagnostics {
        if diagnostic.source.as_deref() != Some("kettu-checker") {
            continue;
        }

        let Some(original) = diagnostic_text_in_range(content, &diagnostic.range) else {
            continue;
        };

        let Some((title, new_text)) =
            build_variant_arity_replacement(&diagnostic.message, original)
        else {
            continue;
        };

        let mut changes = HashMap::new();
        changes.insert(
            uri.clone(),
            vec![TextEdit {
                range: diagnostic.range,
                new_text,
            }],
        );

        let action = CodeAction {
            title,
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            }),
            is_preferred: Some(true),
            ..Default::default()
        };

        actions.push(CodeActionOrCommand::CodeAction(action));
    }

    actions
}

// ============================================================================
// LSP Protocol Implementation
// ============================================================================

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![
                        ".".to_string(),
                        ":".to_string(),
                        "<".to_string(),
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "Kettu LSP initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let content = params.text_document.text;
        let version = params.text_document.version;

        {
            let mut docs = self.documents.write().unwrap();
            docs.insert(uri.clone(), Document::new(content.clone(), version));
        }

        self.publish_diagnostics(uri, &content, version).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;

        if let Some(change) = params.content_changes.into_iter().next() {
            let content = change.text;

            {
                let mut docs = self.documents.write().unwrap();
                docs.insert(uri.clone(), Document::new(content.clone(), version));
            }

            self.publish_diagnostics(uri, &content, version).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let mut docs = self.documents.write().unwrap();
        docs.remove(&params.text_document.uri);
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let docs = self.documents.read().unwrap();
        if let Some(doc) = docs.get(uri) {
            if let Some(symbol) = find_imported_symbol_for_hover(uri, doc, position, &docs) {
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: symbol_markdown(&symbol),
                    }),
                    range: None,
                }));
            }

            if let Some(ast) = doc.ast.as_ref() {
                let inference_ctx =
                    build_inference_context_for_hover(Some(uri), Some(doc), Some(&docs), ast);
                if let Some((markdown, range)) = find_local_hover_info_with_call_returns(
                    &doc.content,
                    ast,
                    position,
                    &inference_ctx,
                ) {
                    return Ok(Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: markdown,
                        }),
                        range,
                    }));
                }
            } else if let Some(symbol) = self.find_symbol_at_position(doc, position) {
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: symbol_markdown(&symbol),
                    }),
                    range: Some(span_to_range(&doc.content, symbol.span)),
                }));
            }
        }
        Ok(None)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let docs = self.documents.read().unwrap();
        if let Some(doc) = docs.get(uri) {
            if let Some(location) = find_imported_definition(uri, doc, position, &docs) {
                return Ok(Some(GotoDefinitionResponse::Scalar(location)));
            }

            if let Some(ast) = doc.ast.as_ref() {
                if let Some(range) = find_definition_range(&doc.content, ast, position) {
                    return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                        uri: uri.clone(),
                        range,
                    })));
                }
            } else if let Some(symbol) = self.find_symbol_at_position(doc, position) {
                let range = span_to_range(&doc.content, symbol.span);
                return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                    uri: uri.clone(),
                    range,
                })));
            }
        }
        Ok(None)
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = &params.text_document.uri;

        let docs = self.documents.read().unwrap();
        if let Some(doc) = docs.get(uri) {
            if let Some(ref ast) = doc.ast {
                let symbols = build_document_symbols(&doc.content, ast);
                return Ok(Some(DocumentSymbolResponse::Nested(symbols)));
            }
        }
        Ok(None)
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;

        let docs = self.documents.read().unwrap();
        if let Some(doc) = docs.get(uri) {
            let mut items = Vec::new();

            // WIT keywords
            let keywords = [
                ("interface", "interface $1 {\n\t$0\n}"),
                ("world", "world $1 {\n\t$0\n}"),
                ("func", "$1: func($2)$0;"),
                ("record", "record $1 {\n\t$0\n}"),
                ("enum", "enum $1 {\n\t$0\n}"),
                ("variant", "variant $1 {\n\t$0\n}"),
                ("flags", "flags $1 {\n\t$0\n}"),
                ("resource", "resource $1;"),
                ("type", "type $1 = $0;"),
                ("use", "use $1.{$0};"),
                ("import", "import $0;"),
                ("export", "export $0;"),
                ("package", "package $0;"),
            ];

            for (label, snippet) in keywords {
                items.push(CompletionItem {
                    label: label.to_string(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    insert_text: Some(snippet.to_string()),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    ..Default::default()
                });
            }

            // Primitive types
            let primitives = [
                "u8", "u16", "u32", "u64", "s8", "s16", "s32", "s64", "f32", "f64", "bool", "char",
                "string",
            ];

            for prim in primitives {
                items.push(CompletionItem {
                    label: prim.to_string(),
                    kind: Some(CompletionItemKind::TYPE_PARAMETER),
                    ..Default::default()
                });
            }

            // Container types
            let containers = [
                ("list", "list<$1>"),
                ("option", "option<$1>"),
                ("result", "result<$1, $2>"),
                ("tuple", "tuple<$1>"),
                ("future", "future<$1>"),
                ("stream", "stream<$1>"),
                ("borrow", "borrow<$1>"),
                ("own", "own<$1>"),
            ];

            for (label, snippet) in containers {
                items.push(CompletionItem {
                    label: label.to_string(),
                    kind: Some(CompletionItemKind::TYPE_PARAMETER),
                    insert_text: Some(snippet.to_string()),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    ..Default::default()
                });
            }

            // User-defined types from AST
            if let Some(ref ast) = doc.ast {
                let symbols = build_symbol_index(ast);
                for sym in symbols {
                    if matches!(
                        sym.kind,
                        SymbolDefKind::Type { .. }
                            | SymbolDefKind::Record
                            | SymbolDefKind::Enum
                            | SymbolDefKind::Variant
                            | SymbolDefKind::Resource
                    ) {
                        items.push(CompletionItem {
                            label: sym.name.clone(),
                            kind: Some(CompletionItemKind::STRUCT),
                            detail: sym.container.map(|c| format!("from {}", c)),
                            ..Default::default()
                        });
                    }
                }
            }

            return Ok(Some(CompletionResponse::Array(items)));
        }
        Ok(None)
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;

        let docs = self.documents.read().unwrap();
        let Some(doc) = docs.get(uri) else {
            return Ok(None);
        };

        let actions = variant_arity_code_actions(uri, &doc.content, &params.context.diagnostics);
        if actions.is_empty() {
            Ok(None)
        } else {
            Ok(Some(actions))
        }
    }
}

// ============================================================================
// Server Entry Point
// ============================================================================

/// Start the LSP server on stdio
pub async fn run_server() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_symbol_index() {
        let source = r#"
            package local:test;
            
            interface host {
                record point {
                    x: s32,
                    y: s32,
                }
                
                log: func(msg: string);
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let symbols = build_symbol_index(&ast);

        assert!(
            symbols.len() >= 3,
            "Should have interface, record, and func"
        );

        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"host"));
        assert!(names.contains(&"point"));
        assert!(names.contains(&"log"));
    }

    #[test]
    fn test_comment_range_detection() {
        let source = "// line comment\ncode\n/// doc comment\n/* block */";
        let ranges = find_comment_ranges(source);
        assert_eq!(ranges.len(), 3);
        assert_eq!(&source[ranges[0].clone()], "// line comment");
        assert_eq!(&source[ranges[1].clone()], "/// doc comment");
        assert_eq!(&source[ranges[2].clone()], "/* block */");
    }

    #[test]
    fn test_comment_errors_filtered() {
        // Parse a file with comments — should produce zero real errors
        let source = "package local:test;\n\n/// Doc comment\n// Regular comment\ninterface my-iface {\n    greet: func(name: string) -> string;\n}\n";
        let (ast, parse_errors) = parse_file(source);
        assert!(ast.is_some(), "Should produce AST");

        let comment_ranges = find_comment_ranges(source);
        let real_errors: Vec<_> = parse_errors
            .iter()
            .filter(|e| !is_in_comment(e.error_position.bytes.start, &comment_ranges))
            .collect();
        assert!(
            real_errors.is_empty(),
            "No real errors expected, but got: {:?}",
            real_errors
        );
    }

    #[test]
    fn test_comment_noise_suppression_for_composition_style_file() {
        let source = r#"// World Composition Example
// Demonstrates interfaces with shared types

package example:composed;

/// Types shared across interfaces
interface shared-types {
    record request {
        method: s32,
        path: s32,
    }
}

/// HTTP handling interface
interface http-handler {
    handle: func(method-code: s32, path-code: s32) -> u16 {
        200;
    }
}
"#;

        let (ast, parse_errors) = parse_file(source);
        assert!(ast.is_some(), "Should produce AST");

        // Parser behavior can evolve: this fixture historically triggered
        // comment-induced parse noise, but newer parser versions may parse cleanly.
        if !parse_errors.is_empty() {
            assert!(
                has_only_comment_induced_parse_errors(source),
                "Expected any parse noise here to be comment-induced"
            );
        }

        let ast_for_check = parse_clean_without_comments(source)
            .or(ast)
            .expect("Should obtain an AST for checker validation");
        let check_errors = check(&ast_for_check);
        assert!(
            !check_errors
                .iter()
                .any(|d| d.message.contains("Unknown interface: cli")),
            "Checker should not report unknown cli interface after stripping comments"
        );
    }

    #[test]
    fn test_document_symbols_hierarchy() {
        let source = r#"package local:test;

interface type-defs {
    record point {
        x: s32,
        y: s32
    }

    enum color {
        red,
        green,
        blue
    }

    type maybe-int = option<s32>;
}

interface math {
    add: func(a: s32, b: s32) -> s32;
    negate: func(x: s32) -> s32;
}

world demo {
    export type-defs;
    export math;
}
"#;

        let (ast, errors) = parse_file(source);
        assert!(
            errors.is_empty(),
            "Should parse without errors: {:?}",
            errors
        );
        let ast = ast.expect("Should parse");
        let symbols = build_document_symbols(source, &ast);

        // Should have 3 top-level symbols: type-defs, math, demo
        assert_eq!(
            symbols.len(),
            3,
            "Expected 3 top-level symbols, got: {:?}",
            symbols.iter().map(|s| &s.name).collect::<Vec<_>>()
        );

        // First: interface type-defs with 3 children
        assert_eq!(symbols[0].name, "type-defs");
        assert_eq!(symbols[0].kind, SymbolKind::INTERFACE);
        let type_defs_children = symbols[0]
            .children
            .as_ref()
            .expect("type-defs should have children");
        assert_eq!(
            type_defs_children.len(),
            3,
            "type-defs should have 3 children (point, color, maybe-int)"
        );
        assert_eq!(type_defs_children[0].name, "point");
        assert_eq!(type_defs_children[0].kind, SymbolKind::STRUCT);
        assert_eq!(type_defs_children[1].name, "color");
        assert_eq!(type_defs_children[1].kind, SymbolKind::ENUM);
        assert_eq!(type_defs_children[2].name, "maybe-int");
        assert_eq!(type_defs_children[2].kind, SymbolKind::TYPE_PARAMETER);

        // Second: interface math with 2 function children
        assert_eq!(symbols[1].name, "math");
        assert_eq!(symbols[1].kind, SymbolKind::INTERFACE);
        let math_children = symbols[1]
            .children
            .as_ref()
            .expect("math should have children");
        assert_eq!(math_children.len(), 2);
        assert_eq!(math_children[0].name, "add");
        assert_eq!(math_children[0].kind, SymbolKind::FUNCTION);
        assert_eq!(math_children[1].name, "negate");
        assert_eq!(math_children[1].kind, SymbolKind::FUNCTION);

        // Third: world demo with 2 export children
        assert_eq!(symbols[2].name, "demo");
        assert_eq!(symbols[2].kind, SymbolKind::MODULE);
        let world_children = symbols[2]
            .children
            .as_ref()
            .expect("demo should have children");
        assert_eq!(world_children.len(), 2);

        // Verify selection_range is within range for all symbols
        for sym in &symbols {
            verify_selection_in_range(sym);
        }
    }

    /// Recursively verify that selection_range ⊆ range for all symbols.
    fn verify_selection_in_range(sym: &DocumentSymbol) {
        let r = &sym.range;
        let s = &sym.selection_range;
        assert!(
            s.start.line > r.start.line
                || (s.start.line == r.start.line && s.start.character >= r.start.character),
            "Symbol '{}': selection start {:?} before range start {:?}",
            sym.name,
            s.start,
            r.start
        );
        assert!(
            s.end.line < r.end.line
                || (s.end.line == r.end.line && s.end.character <= r.end.character),
            "Symbol '{}': selection end {:?} after range end {:?}",
            sym.name,
            s.end,
            r.end
        );
        if let Some(children) = &sym.children {
            for child in children {
                verify_selection_in_range(child);
            }
        }
    }

    #[test]
    fn test_document_symbols_empty_file() {
        let source = "package local:test;\n";
        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");
        let symbols = build_document_symbols(source, &ast);
        assert!(symbols.is_empty(), "Empty file should produce no symbols");
    }

    #[test]
    fn test_offset_position_conversion() {
        let content = "line1\nline2\nline3";

        // Position at start of line2
        let pos = Position::new(1, 0);
        let offset = position_to_offset(content, pos);
        assert_eq!(offset, Some(6)); // After "line1\n"

        // Convert back
        let back = offset_to_position(content, 6);
        assert_eq!(back.line, 1);
        assert_eq!(back.character, 0);
    }

    #[test]
    fn test_identifier_at_position_hyphenated_name() {
        let content = "export http-handler;\n";

        let id_mid = identifier_at_position(content, Position::new(0, 10));
        assert_eq!(id_mid.as_deref(), Some("http-handler"));

        let id_on_semicolon = identifier_at_position(content, Position::new(0, 19));
        assert_eq!(id_on_semicolon.as_deref(), Some("http-handler"));
    }

    #[test]
    fn test_find_definition_range_from_world_export_reference() {
        let source = r#"interface cli {
    run: func(argc: s32) -> s32 {
        argc;
    }
}

world cli-app {
    export cli;
}
"#;

        let (ast, errors) = parse_file(source);
        assert!(
            errors.is_empty(),
            "Should parse without errors: {:?}",
            errors
        );
        let ast = ast.expect("Should parse");

        // Position inside `cli` in `export cli;`
        let range = find_definition_range(source, &ast, Position::new(7, 11))
            .expect("Definition should resolve");

        // Should resolve to line 0 where `interface cli` is declared.
        assert_eq!(range.start.line, 0);
    }

    #[test]
    fn test_document_new_preserves_cli_definition_with_comment_noise() {
        let source = r#"// World Composition Example
// Demonstrates interfaces with shared types

package example:composed;

/// HTTP handling interface
interface http-handler {
    handle: func(method-code: s32, path-code: s32) -> u16 {
        200;
    }
}

/// CLI interface
interface cli {
    run: func(argc: s32) -> s32 {
        argc;
    }
}

/// CLI application world
world cli-app {
    export cli;
}
"#;

        // Raw parse should produce an AST (with or without comment-induced noise,
        // depending on parser version).
        let (raw_ast, raw_errors) = parse_file(source);
        assert!(raw_ast.is_some(), "Raw parse should produce AST");
        if !raw_errors.is_empty() {
            assert!(
                has_only_comment_induced_parse_errors(source),
                "Expected any raw parse noise here to be comment-induced"
            );
        }

        // Document::new should prefer comment-stripped AST for editor features.
        let doc = Document::new(source.to_string(), 1);
        let ast = doc.ast.as_ref().expect("Document should retain AST");
        let symbols = build_symbol_index(ast);
        assert!(
            symbols.iter().any(|s| s.name == "cli"),
            "Document AST should include 'cli' interface symbol"
        );

        let export_line = doc
            .content
            .lines()
            .position(|line| line.contains("export cli;"))
            .expect("Fixture should contain 'export cli;'") as u32;
        let export_col = doc
            .content
            .lines()
            .nth(export_line as usize)
            .and_then(|line| line.find("cli"))
            .expect("Fixture should contain 'cli' in export line") as u32;

        let expected_def_line =
            doc.content
                .lines()
                .position(|line| line.contains("interface cli"))
                .expect("Fixture should contain 'interface cli'") as u32;

        // Go-to-definition on `export cli;` should resolve to interface declaration.
        let range =
            find_definition_range(&doc.content, ast, Position::new(export_line, export_col))
                .expect("Definition should resolve for exported interface reference");
        assert_eq!(range.start.line, expected_def_line);
    }

    #[test]
    fn test_find_imported_definition_for_world_import_interface() {
        use std::fs;

        let temp_root = std::env::temp_dir().join(format!(
            "kettu_lsp_goto_import_interface_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_root);
        fs::create_dir_all(temp_root.join("helper")).expect("create helper dir");

        let main_path = temp_root.join("main.kettu");
        let lib_path = temp_root.join("helper/lib.kettu");

        let main_src = r#"package my:app;

interface app {
    run: func() -> s32 {
        let x = math.add(10, 20);
        x;
    }
}

world app-world {
    import helper:lib/math;
    export app;
}
"#;

        let lib_src = r#"package helper:lib;

interface math {
    add: func(a: s32, b: s32) -> s32 {
        a + b;
    }
}
"#;

        fs::write(&main_path, main_src).expect("write main");
        fs::write(&lib_path, lib_src).expect("write lib");

        let main_uri = Url::from_file_path(&main_path).expect("main uri");
        let lib_uri = Url::from_file_path(&lib_path).expect("lib uri");

        let main_doc = Document::new(main_src.to_string(), 1);
        let lib_doc = Document::new(lib_src.to_string(), 1);

        let docs = HashMap::from([(main_uri.clone(), main_doc), (lib_uri.clone(), lib_doc)]);
        let source_doc = docs.get(&main_uri).expect("main doc in map");

        let import_line = main_src
            .lines()
            .position(|line| line.contains("import helper:lib/math;"))
            .expect("import line exists") as u32;
        let import_col = main_src
            .lines()
            .nth(import_line as usize)
            .and_then(|line| line.find("math"))
            .expect("math token exists") as u32;

        let location = find_imported_definition(
            &main_uri,
            source_doc,
            Position::new(import_line, import_col),
            &docs,
        )
        .expect("imported interface definition should resolve");

        assert_eq!(location.uri, lib_uri);
        let expected_line = lib_src
            .lines()
            .position(|line| line.contains("interface math"))
            .expect("interface line exists") as u32;
        assert_eq!(location.range.start.line, expected_line);

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_find_imported_definition_for_qualified_member_call() {
        use std::fs;

        let temp_root = std::env::temp_dir().join(format!(
            "kettu_lsp_goto_import_member_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_root);
        fs::create_dir_all(temp_root.join("helper")).expect("create helper dir");

        let main_path = temp_root.join("main.kettu");
        let lib_path = temp_root.join("helper/lib.kettu");

        let main_src = r#"package my:app;

interface app {
    run: func() -> s32 {
        let x = math.add(10, 20);
        x;
    }
}

world app-world {
    import helper:lib/math;
    export app;
}
"#;

        let lib_src = r#"package helper:lib;

interface math {
    add: func(a: s32, b: s32) -> s32 {
        a + b;
    }

    multiply: func(a: s32, b: s32) -> s32 {
        a * b;
    }
}
"#;

        fs::write(&main_path, main_src).expect("write main");
        fs::write(&lib_path, lib_src).expect("write lib");

        let main_uri = Url::from_file_path(&main_path).expect("main uri");
        let lib_uri = Url::from_file_path(&lib_path).expect("lib uri");

        let main_doc = Document::new(main_src.to_string(), 1);
        let lib_doc = Document::new(lib_src.to_string(), 1);

        let docs = HashMap::from([(main_uri.clone(), main_doc), (lib_uri.clone(), lib_doc)]);
        let source_doc = docs.get(&main_uri).expect("main doc in map");

        let call_line = main_src
            .lines()
            .position(|line| line.contains("math.add(10, 20)"))
            .expect("call line exists") as u32;
        let add_col = main_src
            .lines()
            .nth(call_line as usize)
            .and_then(|line| line.find("add"))
            .expect("add token exists") as u32;

        let location = find_imported_definition(
            &main_uri,
            source_doc,
            Position::new(call_line, add_col),
            &docs,
        )
        .expect("imported member definition should resolve");

        assert_eq!(location.uri, lib_uri);
        let expected_line = lib_src
            .lines()
            .position(|line| line.contains("add: func"))
            .expect("add line exists") as u32;
        assert_eq!(location.range.start.line, expected_line);

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_find_imported_definition_for_top_level_use_alias() {
        use std::fs;

        let temp_root = std::env::temp_dir().join(format!(
            "kettu_lsp_goto_top_level_use_alias_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_root);
        fs::create_dir_all(temp_root.join("helper")).expect("create helper dir");

        let main_path = temp_root.join("main.kettu");
        let lib_path = temp_root.join("helper/lib.kettu");

        let main_src = r#"package my:app;

use helper:lib/math as hmath;

interface app {
    run: func() -> s32 {
        let x = hmath.add(10, 20);
        x;
    }
}
"#;

        let lib_src = r#"package helper:lib;

interface math {
    add: func(a: s32, b: s32) -> s32 {
        a + b;
    }
}
"#;

        fs::write(&main_path, main_src).expect("write main");
        fs::write(&lib_path, lib_src).expect("write lib");

        let main_uri = Url::from_file_path(&main_path).expect("main uri");
        let lib_uri = Url::from_file_path(&lib_path).expect("lib uri");

        let main_doc = Document::new(main_src.to_string(), 1);
        let lib_doc = Document::new(lib_src.to_string(), 1);

        let docs = HashMap::from([(main_uri.clone(), main_doc), (lib_uri.clone(), lib_doc)]);
        let source_doc = docs.get(&main_uri).expect("main doc in map");

        let use_line = main_src
            .lines()
            .position(|line| line.contains("use helper:lib/math as hmath;"))
            .expect("use line exists") as u32;
        let alias_col = main_src
            .lines()
            .nth(use_line as usize)
            .and_then(|line| line.find("hmath"))
            .expect("alias token exists") as u32;

        let alias_location = find_imported_definition(
            &main_uri,
            source_doc,
            Position::new(use_line, alias_col),
            &docs,
        )
        .expect("alias should resolve to imported interface");

        assert_eq!(alias_location.uri, lib_uri);
        let iface_line = lib_src
            .lines()
            .position(|line| line.contains("interface math"))
            .expect("interface line exists") as u32;
        assert_eq!(alias_location.range.start.line, iface_line);

        let call_line = main_src
            .lines()
            .position(|line| line.contains("hmath.add(10, 20)"))
            .expect("call line exists") as u32;
        let add_col = main_src
            .lines()
            .nth(call_line as usize)
            .and_then(|line| line.find("add"))
            .expect("add token exists") as u32;

        let member_location = find_imported_definition(
            &main_uri,
            source_doc,
            Position::new(call_line, add_col),
            &docs,
        )
        .expect("member should resolve to imported function");

        assert_eq!(member_location.uri, lib_uri);
        let add_line = lib_src
            .lines()
            .position(|line| line.contains("add: func"))
            .expect("add line exists") as u32;
        assert_eq!(member_location.range.start.line, add_line);

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_find_imported_definition_for_top_level_use_without_open_target_doc() {
        use std::fs;

        let temp_root = std::env::temp_dir().join(format!(
            "kettu_lsp_goto_top_level_use_unopened_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_root);
        fs::create_dir_all(temp_root.join("helper")).expect("create helper dir");

        let main_path = temp_root.join("main.kettu");
        let lib_path = temp_root.join("helper/lib.kettu");

        let main_src = r#"package my:app;

use helper:lib/math;

interface app {
    run: func() -> s32 {
        let x = math.add(10, 20);
        x;
    }
}
"#;

        let lib_src = r#"package helper:lib;

interface math {
    add: func(a: s32, b: s32) -> s32 {
        a + b;
    }
}
"#;

        fs::write(&main_path, main_src).expect("write main");
        fs::write(&lib_path, lib_src).expect("write lib");

        let main_uri = Url::from_file_path(&main_path).expect("main uri");
        let lib_uri = Url::from_file_path(&lib_path).expect("lib uri");

        // Keep only source doc open to force find_imported_definition to load
        // target file from disk.
        let main_doc = Document::new(main_src.to_string(), 1);
        let docs = HashMap::from([(main_uri.clone(), main_doc)]);
        let source_doc = docs.get(&main_uri).expect("main doc in map");

        let call_line = main_src
            .lines()
            .position(|line| line.contains("math.add(10, 20)"))
            .expect("call line exists") as u32;
        let add_col = main_src
            .lines()
            .nth(call_line as usize)
            .and_then(|line| line.find("add"))
            .expect("add token exists") as u32;

        let location = find_imported_definition(
            &main_uri,
            source_doc,
            Position::new(call_line, add_col),
            &docs,
        )
        .expect("member should resolve from unopened target file");

        assert_eq!(location.uri, lib_uri);
        let expected_line = lib_src
            .lines()
            .position(|line| line.contains("add: func"))
            .expect("add line exists") as u32;
        assert_eq!(location.range.start.line, expected_line);

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_find_imported_definition_for_qualified_member_with_whitespace() {
        use std::fs;

        let temp_root = std::env::temp_dir().join(format!(
            "kettu_lsp_goto_import_member_whitespace_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_root);
        fs::create_dir_all(temp_root.join("helper")).expect("create helper dir");

        let main_path = temp_root.join("main.kettu");
        let lib_path = temp_root.join("helper/lib.kettu");

        let main_src = r#"package my:app;

interface app {
    run: func() -> s32 {
        let x = math . add(10, 20);
        x;
    }
}

world app-world {
    import helper:lib/math;
    export app;
}
"#;

        let lib_src = r#"package helper:lib;

interface math {
    add: func(a: s32, b: s32) -> s32 {
        a + b;
    }
}
"#;

        fs::write(&main_path, main_src).expect("write main");
        fs::write(&lib_path, lib_src).expect("write lib");

        let main_uri = Url::from_file_path(&main_path).expect("main uri");
        let lib_uri = Url::from_file_path(&lib_path).expect("lib uri");

        let main_doc = Document::new(main_src.to_string(), 1);
        let lib_doc = Document::new(lib_src.to_string(), 1);

        let docs = HashMap::from([(main_uri.clone(), main_doc), (lib_uri.clone(), lib_doc)]);
        let source_doc = docs.get(&main_uri).expect("main doc in map");

        let call_line = main_src
            .lines()
            .position(|line| line.contains("math . add(10, 20)"))
            .expect("call line exists") as u32;

        let add_col = main_src
            .lines()
            .nth(call_line as usize)
            .and_then(|line| line.find("add"))
            .expect("add token exists") as u32;

        let add_location = find_imported_definition(
            &main_uri,
            source_doc,
            Position::new(call_line, add_col),
            &docs,
        )
        .expect("spaced qualified member should resolve");

        assert_eq!(add_location.uri, lib_uri);
        let add_line = lib_src
            .lines()
            .position(|line| line.contains("add: func"))
            .expect("add line exists") as u32;
        assert_eq!(add_location.range.start.line, add_line);

        let math_col = main_src
            .lines()
            .nth(call_line as usize)
            .and_then(|line| line.find("math"))
            .expect("math token exists") as u32;

        let math_location = find_imported_definition(
            &main_uri,
            source_doc,
            Position::new(call_line, math_col),
            &docs,
        )
        .expect("spaced qualifier should resolve");

        assert_eq!(math_location.uri, lib_uri);
        let iface_line = lib_src
            .lines()
            .position(|line| line.contains("interface math"))
            .expect("interface line exists") as u32;
        assert_eq!(math_location.range.start.line, iface_line);

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_find_imported_definition_returns_none_for_unknown_qualifier() {
        use std::fs;

        let temp_root = std::env::temp_dir().join(format!(
            "kettu_lsp_goto_unknown_qualifier_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_root);
        fs::create_dir_all(temp_root.join("helper")).expect("create helper dir");

        let main_path = temp_root.join("main.kettu");
        let lib_path = temp_root.join("helper/lib.kettu");

        let main_src = r#"package my:app;

interface app {
    run: func() -> s32 {
        let x = nope.add(10, 20);
        x;
    }
}

world app-world {
    import helper:lib/math;
    export app;
}
"#;

        let lib_src = r#"package helper:lib;

interface math {
    add: func(a: s32, b: s32) -> s32 {
        a + b;
    }
}
"#;

        fs::write(&main_path, main_src).expect("write main");
        fs::write(&lib_path, lib_src).expect("write lib");

        let main_uri = Url::from_file_path(&main_path).expect("main uri");
        let lib_uri = Url::from_file_path(&lib_path).expect("lib uri");

        let main_doc = Document::new(main_src.to_string(), 1);
        let lib_doc = Document::new(lib_src.to_string(), 1);
        let docs = HashMap::from([(main_uri.clone(), main_doc), (lib_uri.clone(), lib_doc)]);

        let source_doc = docs.get(&main_uri).expect("main doc in map");
        let call_line = main_src
            .lines()
            .position(|line| line.contains("nope.add(10, 20)"))
            .expect("call line exists") as u32;
        let add_col = main_src
            .lines()
            .nth(call_line as usize)
            .and_then(|line| line.find("add"))
            .expect("add token exists") as u32;

        let location = find_imported_definition(
            &main_uri,
            source_doc,
            Position::new(call_line, add_col),
            &docs,
        );

        assert!(location.is_none(), "unknown qualifier should not resolve");

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_find_imported_definition_returns_none_for_unknown_member() {
        use std::fs;

        let temp_root = std::env::temp_dir().join(format!(
            "kettu_lsp_goto_unknown_member_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_root);
        fs::create_dir_all(temp_root.join("helper")).expect("create helper dir");

        let main_path = temp_root.join("main.kettu");
        let lib_path = temp_root.join("helper/lib.kettu");

        let main_src = r#"package my:app;

interface app {
    run: func() -> s32 {
        let x = math.nope(10, 20);
        x;
    }
}

world app-world {
    import helper:lib/math;
    export app;
}
"#;

        let lib_src = r#"package helper:lib;

interface math {
    add: func(a: s32, b: s32) -> s32 {
        a + b;
    }
}
"#;

        fs::write(&main_path, main_src).expect("write main");
        fs::write(&lib_path, lib_src).expect("write lib");

        let main_uri = Url::from_file_path(&main_path).expect("main uri");
        let lib_uri = Url::from_file_path(&lib_path).expect("lib uri");

        let main_doc = Document::new(main_src.to_string(), 1);
        let lib_doc = Document::new(lib_src.to_string(), 1);
        let docs = HashMap::from([(main_uri.clone(), main_doc), (lib_uri.clone(), lib_doc)]);

        let source_doc = docs.get(&main_uri).expect("main doc in map");
        let call_line = main_src
            .lines()
            .position(|line| line.contains("math.nope(10, 20)"))
            .expect("call line exists") as u32;
        let nope_col = main_src
            .lines()
            .nth(call_line as usize)
            .and_then(|line| line.find("nope"))
            .expect("nope token exists") as u32;

        let location = find_imported_definition(
            &main_uri,
            source_doc,
            Position::new(call_line, nope_col),
            &docs,
        );

        assert!(location.is_none(), "unknown member should not resolve");

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_find_imported_symbol_for_hover_world_import_interface_and_member() {
        use std::fs;

        let temp_root = std::env::temp_dir().join(format!(
            "kettu_lsp_hover_world_import_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_root);
        fs::create_dir_all(temp_root.join("helper")).expect("create helper dir");

        let main_path = temp_root.join("main.kettu");
        let lib_path = temp_root.join("helper/lib.kettu");

        let main_src = r#"package my:app;

interface app {
    run: func() -> s32 {
        let x = math.add(10, 20);
        x;
    }
}

world app-world {
    import helper:lib/math;
    export app;
}
"#;

        let lib_src = r#"package helper:lib;

interface math {
    add: func(a: s32, b: s32) -> s32 {
        a + b;
    }
}
"#;

        fs::write(&main_path, main_src).expect("write main");
        fs::write(&lib_path, lib_src).expect("write lib");

        let main_uri = Url::from_file_path(&main_path).expect("main uri");
        let lib_uri = Url::from_file_path(&lib_path).expect("lib uri");

        let main_doc = Document::new(main_src.to_string(), 1);
        let lib_doc = Document::new(lib_src.to_string(), 1);
        let docs = HashMap::from([(main_uri.clone(), main_doc), (lib_uri.clone(), lib_doc)]);

        let source_doc = docs.get(&main_uri).expect("main doc in map");
        let call_line = main_src
            .lines()
            .position(|line| line.contains("math.add(10, 20)"))
            .expect("call line exists") as u32;

        let math_col = main_src
            .lines()
            .nth(call_line as usize)
            .and_then(|line| line.find("math"))
            .expect("math token exists") as u32;

        let qualifier_symbol = find_imported_symbol_for_hover(
            &main_uri,
            source_doc,
            Position::new(call_line, math_col),
            &docs,
        )
        .expect("hover should resolve imported interface symbol");

        assert!(matches!(qualifier_symbol.kind, SymbolDefKind::Interface));
        assert_eq!(qualifier_symbol.name, "math");

        let add_col = main_src
            .lines()
            .nth(call_line as usize)
            .and_then(|line| line.find("add"))
            .expect("add token exists") as u32;

        let member_symbol = find_imported_symbol_for_hover(
            &main_uri,
            source_doc,
            Position::new(call_line, add_col),
            &docs,
        )
        .expect("hover should resolve imported member symbol");

        assert!(matches!(member_symbol.kind, SymbolDefKind::Func { .. }));
        assert_eq!(member_symbol.name, "add");

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_find_imported_symbol_for_hover_top_level_use_alias_member() {
        use std::fs;

        let temp_root = std::env::temp_dir().join(format!(
            "kettu_lsp_hover_top_level_alias_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_root);
        fs::create_dir_all(temp_root.join("helper")).expect("create helper dir");

        let main_path = temp_root.join("main.kettu");
        let lib_path = temp_root.join("helper/lib.kettu");

        let main_src = r#"package my:app;

use helper:lib/math as hmath;

interface app {
    run: func() -> s32 {
        let x = hmath.add(10, 20);
        x;
    }
}
"#;

        let lib_src = r#"package helper:lib;

interface math {
    add: func(a: s32, b: s32) -> s32 {
        a + b;
    }
}
"#;

        fs::write(&main_path, main_src).expect("write main");
        fs::write(&lib_path, lib_src).expect("write lib");

        let main_uri = Url::from_file_path(&main_path).expect("main uri");
        let lib_uri = Url::from_file_path(&lib_path).expect("lib uri");

        let main_doc = Document::new(main_src.to_string(), 1);
        let lib_doc = Document::new(lib_src.to_string(), 1);
        let docs = HashMap::from([(main_uri.clone(), main_doc), (lib_uri.clone(), lib_doc)]);

        let source_doc = docs.get(&main_uri).expect("main doc in map");
        let call_line = main_src
            .lines()
            .position(|line| line.contains("hmath.add(10, 20)"))
            .expect("call line exists") as u32;

        let add_col = main_src
            .lines()
            .nth(call_line as usize)
            .and_then(|line| line.find("add"))
            .expect("add token exists") as u32;

        let symbol = find_imported_symbol_for_hover(
            &main_uri,
            source_doc,
            Position::new(call_line, add_col),
            &docs,
        )
        .expect("hover should resolve aliased imported member symbol");

        assert!(matches!(symbol.kind, SymbolDefKind::Func { .. }));
        assert_eq!(symbol.name, "add");

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_find_imported_symbol_for_hover_returns_none_for_unknown_member() {
        use std::fs;

        let temp_root = std::env::temp_dir().join(format!(
            "kettu_lsp_hover_unknown_member_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_root);
        fs::create_dir_all(temp_root.join("helper")).expect("create helper dir");

        let main_path = temp_root.join("main.kettu");
        let lib_path = temp_root.join("helper/lib.kettu");

        let main_src = r#"package my:app;

interface app {
    run: func() -> s32 {
        let x = math.nope(10, 20);
        x;
    }
}

world app-world {
    import helper:lib/math;
    export app;
}
"#;

        let lib_src = r#"package helper:lib;

interface math {
    add: func(a: s32, b: s32) -> s32 {
        a + b;
    }
}
"#;

        fs::write(&main_path, main_src).expect("write main");
        fs::write(&lib_path, lib_src).expect("write lib");

        let main_uri = Url::from_file_path(&main_path).expect("main uri");
        let lib_uri = Url::from_file_path(&lib_path).expect("lib uri");

        let main_doc = Document::new(main_src.to_string(), 1);
        let lib_doc = Document::new(lib_src.to_string(), 1);
        let docs = HashMap::from([(main_uri.clone(), main_doc), (lib_uri.clone(), lib_doc)]);

        let source_doc = docs.get(&main_uri).expect("main doc in map");
        let call_line = main_src
            .lines()
            .position(|line| line.contains("math.nope(10, 20)"))
            .expect("call line exists") as u32;
        let nope_col = main_src
            .lines()
            .nth(call_line as usize)
            .and_then(|line| line.find("nope"))
            .expect("nope token exists") as u32;

        let symbol = find_imported_symbol_for_hover(
            &main_uri,
            source_doc,
            Position::new(call_line, nope_col),
            &docs,
        );

        assert!(symbol.is_none(), "unknown imported member should not hover");

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_find_local_hover_info_prefers_param_in_expression() {
        let source = r#"package local:test;

interface math {
    multiply: func(a: s32, b: s32) -> s32 {
        a * b;
    }
}
"#;

        let (ast, errors) = parse_file(source);
        assert!(errors.is_empty(), "Should parse cleanly: {:?}", errors);
        let ast = ast.expect("Should parse");

        let line = source
            .lines()
            .position(|l| l.contains("a * b;"))
            .expect("expression line exists") as u32;
        let col = source
            .lines()
            .nth(line as usize)
            .and_then(|l| l.find("b"))
            .expect("b exists") as u32;

        let hover = find_local_hover_info(source, &ast, Position::new(line, col))
            .expect("param hover should resolve");

        assert!(hover.0.contains("**param**"));
        assert!(hover.0.contains("`b`"));
        assert!(hover.0.contains("s32"));
    }

    #[test]
    fn test_find_local_hover_info_identifier_without_symbol_returns_none() {
        let source = r#"package local:test;

interface math {
    multiply: func(a: s32, b: s32) -> s32 {
        y;
    }
}
"#;

        let (ast, errors) = parse_file(source);
        assert!(errors.is_empty(), "Should parse cleanly: {:?}", errors);
        let ast = ast.expect("Should parse");

        let line = source
            .lines()
            .position(|l| l.trim() == "y;")
            .expect("y line exists") as u32;
        let col = source
            .lines()
            .nth(line as usize)
            .and_then(|l| l.find("y"))
            .expect("y exists") as u32;

        let hover = find_local_hover_info(source, &ast, Position::new(line, col));
        assert!(
            hover.is_none(),
            "unresolved expression identifier should not fall back to function-level hover"
        );
    }

    #[test]
    fn test_find_local_hover_info_infers_let_type_from_binary_expression() {
        let source = r#"package local:test;

interface math {
    multiply: func(a: s32, b: s32) -> s32 {
        let x = a * b;
        x;
    }
}
"#;

        let (ast, errors) = parse_file(source);
        assert!(errors.is_empty(), "Should parse cleanly: {:?}", errors);
        let ast = ast.expect("Should parse");

        let line = source
            .lines()
            .position(|l| l.trim() == "x;")
            .expect("x line exists") as u32;
        let col = source
            .lines()
            .nth(line as usize)
            .and_then(|l| l.find("x"))
            .expect("x exists") as u32;

        let hover = find_local_hover_info(source, &ast, Position::new(line, col))
            .expect("local let hover should resolve inferred type");
        assert!(hover.0.contains("**let**"));
        assert!(hover.0.contains("`x`"));
        assert!(hover.0.contains("s32"));
    }

    #[test]
    fn test_find_local_hover_info_infers_let_type_from_local_call() {
        let source = r#"package local:test;

interface math {
    multiply: func(a: s32, b: s32) -> s32 {
        a * b;
    }

    run: func(a: s32, b: s32) -> s32 {
        let x = multiply(a, b);
        x;
    }
}
"#;

        let (ast, errors) = parse_file(source);
        assert!(errors.is_empty(), "Should parse cleanly: {:?}", errors);
        let ast = ast.expect("Should parse");

        let line = source
            .lines()
            .position(|l| l.trim() == "x;")
            .expect("x line exists") as u32;
        let col = source
            .lines()
            .nth(line as usize)
            .and_then(|l| l.find("x"))
            .expect("x exists") as u32;

        let hover = find_local_hover_info(source, &ast, Position::new(line, col))
            .expect("local let hover should resolve inferred type");
        assert!(hover.0.contains("**let**"));
        assert!(hover.0.contains("`x`"));
        assert!(hover.0.contains("s32"));
    }

    #[test]
    fn test_find_local_hover_info_infers_let_type_from_imported_qualified_call() {
        use std::fs;

        let temp_root = std::env::temp_dir().join(format!(
            "kettu_lsp_hover_infer_imported_call_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_root);
        fs::create_dir_all(temp_root.join("helper")).expect("create helper dir");

        let main_path = temp_root.join("main.kettu");
        let lib_path = temp_root.join("helper/lib.kettu");

        let main_src = r#"package my:app;

interface app {
    run: func() -> s32 {
        let x = math.add(10, 20);
        x;
    }
}

world app-world {
    import helper:lib/math;
    export app;
}
"#;

        let lib_src = r#"package helper:lib;

interface math {
    add: func(a: s32, b: s32) -> s32 {
        a + b;
    }
}
"#;

        fs::write(&main_path, main_src).expect("write main");
        fs::write(&lib_path, lib_src).expect("write lib");

        let main_uri = Url::from_file_path(&main_path).expect("main uri");
        let lib_uri = Url::from_file_path(&lib_path).expect("lib uri");

        let main_doc = Document::new(main_src.to_string(), 1);
        let lib_doc = Document::new(lib_src.to_string(), 1);
        let docs = HashMap::from([(main_uri.clone(), main_doc), (lib_uri.clone(), lib_doc)]);

        let source_doc = docs.get(&main_uri).expect("main doc in map");
        let source_ast = source_doc.ast.as_ref().expect("main ast");

        let inference_ctx = build_inference_context_for_hover(
            Some(&main_uri),
            Some(source_doc),
            Some(&docs),
            source_ast,
        );

        let line = main_src
            .lines()
            .position(|l| l.trim() == "x;")
            .expect("x line exists") as u32;
        let col = main_src
            .lines()
            .nth(line as usize)
            .and_then(|l| l.find("x"))
            .expect("x exists") as u32;

        let hover = find_local_hover_info_with_call_returns(
            &source_doc.content,
            source_ast,
            Position::new(line, col),
            &inference_ctx,
        )
        .expect("imported call let hover should resolve inferred type");

        assert!(hover.0.contains("**let**"));
        assert!(hover.0.contains("`x`"));
        assert!(hover.0.contains("s32"));

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_find_local_hover_info_infers_let_type_from_record_field_access() {
        let source = r#"package local:test;

interface math {
    record point {
        x: s32,
        y: s32,
    }

    run: func(p: point) -> s32 {
        let x = p.x;
        x;
    }
}
"#;

        let (ast, errors) = parse_file(source);
        assert!(errors.is_empty(), "Should parse cleanly: {:?}", errors);
        let ast = ast.expect("Should parse");

        let line = source
            .lines()
            .position(|l| l.trim() == "x;")
            .expect("x line exists") as u32;
        let col = source
            .lines()
            .nth(line as usize)
            .and_then(|l| l.find("x"))
            .expect("x exists") as u32;

        let hover = find_local_hover_info(source, &ast, Position::new(line, col))
            .expect("field-access let hover should resolve inferred type");
        assert!(hover.0.contains("**let**"));
        assert!(hover.0.contains("`x`"));
        assert!(hover.0.contains("s32"));
    }

    #[test]
    fn test_find_local_hover_info_infers_let_type_from_if_expression() {
        let source = r#"package local:test;

interface math {
    run: func() -> s32 {
        let x = if true {
            1;
        } else {
            2;
        };
        x;
    }
}
"#;

        let (ast, errors) = parse_file(source);
        assert!(errors.is_empty(), "Should parse cleanly: {:?}", errors);
        let ast = ast.expect("Should parse");

        let line = source
            .lines()
            .position(|l| l.trim() == "x;")
            .expect("x line exists") as u32;
        let col = source
            .lines()
            .nth(line as usize)
            .and_then(|l| l.find("x"))
            .expect("x exists") as u32;

        let hover = find_local_hover_info(source, &ast, Position::new(line, col))
            .expect("if-expression let hover should resolve inferred type");
        assert!(hover.0.contains("**let**"));
        assert!(hover.0.contains("`x`"));
        assert!(hover.0.contains("s32"));
    }

    #[test]
    fn test_find_local_hover_info_infers_let_type_from_match_expression() {
        let source = r#"package local:test;

interface math {
    run: func() -> s32 {
        let v = #some(1);
        let x = match v {
            #some(_) => 1,
            #none => 2,
        };
        x;
    }
}
"#;

        let (ast, errors) = parse_file(source);
        assert!(errors.is_empty(), "Should parse cleanly: {:?}", errors);
        let ast = ast.expect("Should parse");

        let line = source
            .lines()
            .position(|l| l.trim() == "x;")
            .expect("x line exists") as u32;
        let col = source
            .lines()
            .nth(line as usize)
            .and_then(|l| l.find("x"))
            .expect("x exists") as u32;

        let hover = find_local_hover_info(source, &ast, Position::new(line, col))
            .expect("match-expression let hover should resolve inferred type");
        assert!(hover.0.contains("**let**"));
        assert!(hover.0.contains("`x`"));
        assert!(hover.0.contains("s32"));
    }

    #[test]
    fn test_find_local_hover_info_infers_let_type_from_match_binding() {
        let source = r#"package local:test;

interface math {
    run: func(v: option<s32>) -> s32 {
        let x = match v {
            #some(n) => n,
            #none => 0,
        };
        x;
    }
}
"#;

        let (ast, errors) = parse_file(source);
        assert!(errors.is_empty(), "Should parse cleanly: {:?}", errors);
        let ast = ast.expect("Should parse");

        let line = source
            .lines()
            .position(|l| l.trim() == "x;")
            .expect("x line exists") as u32;
        let col = source
            .lines()
            .nth(line as usize)
            .and_then(|l| l.find("x"))
            .expect("x exists") as u32;

        let hover = find_local_hover_info(source, &ast, Position::new(line, col))
            .expect("match-binding let hover should resolve inferred type");
        assert!(hover.0.contains("**let**"));
        assert!(hover.0.contains("`x`"));
        assert!(hover.0.contains("s32"));
    }

    #[test]
    fn test_find_local_hover_info_infers_let_type_from_qualified_variant_constructor() {
        let source = r#"package local:test;

interface math {
    variant my-result {
        ok(s32),
        err,
    }

    run: func() {
        let v = my-result#ok(42);
        v;
    }
}
"#;

        let (ast, errors) = parse_file(source);
        assert!(errors.is_empty(), "Should parse cleanly: {:?}", errors);
        let ast = ast.expect("Should parse");

        let line = source
            .lines()
            .position(|l| l.trim() == "v;")
            .expect("v line exists") as u32;
        let col = source
            .lines()
            .nth(line as usize)
            .and_then(|l| l.find("v"))
            .expect("v exists") as u32;

        let hover = find_local_hover_info(source, &ast, Position::new(line, col))
            .expect("qualified variant constructor let hover should resolve inferred type");
        assert!(hover.0.contains("**let**"));
        assert!(hover.0.contains("`v`"));
        assert!(hover.0.contains("my-result"));
    }

    #[test]
    fn test_find_local_hover_info_no_inference_for_qualified_variant_unexpected_payload() {
        let source = r#"package local:test;

interface math {
    variant switch {
        on,
        off,
    }

    run: func() {
        let v = switch#on(1);
        v;
    }
}
"#;

        let (ast, errors) = parse_file(source);
        assert!(errors.is_empty(), "Should parse cleanly: {:?}", errors);
        let ast = ast.expect("Should parse");

        let line = source
            .lines()
            .position(|l| l.trim() == "v;")
            .expect("v line exists") as u32;
        let col = source
            .lines()
            .nth(line as usize)
            .and_then(|l| l.find("v"))
            .expect("v exists") as u32;

        let hover = find_local_hover_info(source, &ast, Position::new(line, col));
        assert!(
            hover.is_none(),
            "invalid qualified constructor arity should not infer local let hover"
        );
    }

    #[test]
    fn test_find_local_hover_info_infers_let_type_from_qualified_match_binding() {
        let source = r#"package local:test;

interface math {
    variant my-result {
        ok(s32),
        err,
    }

    run: func(v: my-result) -> s32 {
        let x = match v {
            my-result#ok(n) => n,
            my-result#err => 0,
        };
        x;
    }
}
"#;

        let (ast, errors) = parse_file(source);
        assert!(errors.is_empty(), "Should parse cleanly: {:?}", errors);
        let ast = ast.expect("Should parse");

        let line = source
            .lines()
            .position(|l| l.trim() == "x;")
            .expect("x line exists") as u32;
        let col = source
            .lines()
            .nth(line as usize)
            .and_then(|l| l.find("x"))
            .expect("x exists") as u32;

        let hover = find_local_hover_info(source, &ast, Position::new(line, col))
            .expect("qualified match-binding let hover should resolve inferred type");
        assert!(hover.0.contains("**let**"));
        assert!(hover.0.contains("`x`"));
        assert!(hover.0.contains("s32"));
    }

    #[test]
    fn test_find_local_hover_info_no_inference_for_qualified_match_binding_on_plain_case() {
        let source = r#"package local:test;

interface math {
    variant switch {
        on,
        off,
    }

    run: func(v: switch) {
        let x = match v {
            switch#on(n) => n,
            switch#off => 0,
        };
        x;
    }
}
"#;

        let (ast, errors) = parse_file(source);
        assert!(errors.is_empty(), "Should parse cleanly: {:?}", errors);
        let ast = ast.expect("Should parse");

        let line = source
            .lines()
            .position(|l| l.trim() == "x;")
            .expect("x line exists") as u32;
        let col = source
            .lines()
            .nth(line as usize)
            .and_then(|l| l.find("x"))
            .expect("x exists") as u32;

        let hover = find_local_hover_info(source, &ast, Position::new(line, col));
        assert!(
            hover.is_none(),
            "invalid qualified pattern binding should not infer match result type hover"
        );
    }

    #[test]
    fn test_find_local_hover_info_infers_let_type_from_try_expression() {
        let source = r#"package local:test;

interface effects {
    run: func(v: option<s32>) {
        let x = v?;
        x;
    }
}
"#;

        let (ast, errors) = parse_file(source);
        assert!(errors.is_empty(), "Should parse cleanly: {:?}", errors);
        let ast = ast.expect("Should parse");

        let line = source
            .lines()
            .position(|l| l.trim() == "x;")
            .expect("x line exists") as u32;
        let col = source
            .lines()
            .nth(line as usize)
            .and_then(|l| l.find("x"))
            .expect("x exists") as u32;

        let hover = find_local_hover_info(source, &ast, Position::new(line, col))
            .expect("try-expression let hover should resolve inferred type");
        assert!(hover.0.contains("**let**"));
        assert!(hover.0.contains("`x`"));
        assert!(hover.0.contains("s32"));
    }

    #[test]
    fn test_find_local_hover_info_infers_let_type_from_await_expression() {
        let source = r#"package local:test;

interface effects {
    run: func(f: future<s32>) {
        let x = await f;
        x;
    }
}
"#;

        let (ast, errors) = parse_file(source);
        assert!(errors.is_empty(), "Should parse cleanly: {:?}", errors);
        let ast = ast.expect("Should parse");

        let line = source
            .lines()
            .position(|l| l.trim() == "x;")
            .expect("x line exists") as u32;
        let col = source
            .lines()
            .nth(line as usize)
            .and_then(|l| l.find("x"))
            .expect("x exists") as u32;

        let hover = find_local_hover_info(source, &ast, Position::new(line, col))
            .expect("await-expression let hover should resolve inferred type");
        assert!(hover.0.contains("**let**"));
        assert!(hover.0.contains("`x`"));
        assert!(hover.0.contains("s32"));
    }

    #[test]
    fn test_find_local_hover_info_infers_let_type_from_optional_chain_expression() {
        let source = r#"package local:test;

interface effects {
    record point {
        x: s32,
    }

    run: func(p: option<point>) {
        let ox = p?.x;
        ox;
    }
}
"#;

        let (ast, errors) = parse_file(source);
        assert!(errors.is_empty(), "Should parse cleanly: {:?}", errors);
        let ast = ast.expect("Should parse");

        let line = source
            .lines()
            .position(|l| l.trim() == "ox;")
            .expect("ox line exists") as u32;
        let col = source
            .lines()
            .nth(line as usize)
            .and_then(|l| l.find("ox"))
            .expect("ox exists") as u32;

        let hover = find_local_hover_info(source, &ast, Position::new(line, col))
            .expect("optional-chain let hover should resolve inferred type");
        assert!(hover.0.contains("**let**"));
        assert!(hover.0.contains("`ox`"));
        assert!(hover.0.contains("option<s32>"));
    }

    #[test]
    fn test_build_variant_arity_replacement_constructor_requires_payload() {
        let (title, replacement) = build_variant_arity_replacement(
            "Case 'my-result#ok' requires a payload",
            "my-result#ok",
        )
        .expect("replacement should be produced");

        assert_eq!(title, "Add payload argument");
        assert_eq!(replacement, "my-result#ok(/* payload */)");
    }

    #[test]
    fn test_build_variant_arity_replacement_constructor_remove_payload() {
        let (title, replacement) = build_variant_arity_replacement(
            "Case 'switch#on' does not accept a payload",
            "switch#on(1)",
        )
        .expect("replacement should be produced");

        assert_eq!(title, "Remove payload argument");
        assert_eq!(replacement, "switch#on");
    }

    #[test]
    fn test_build_variant_arity_replacement_pattern_requires_binding() {
        let (title, replacement) = build_variant_arity_replacement(
            "Case 'my-result#ok' pattern requires a binding for payload",
            "my-result#ok",
        )
        .expect("replacement should be produced");

        assert_eq!(title, "Add payload binding to pattern");
        assert_eq!(replacement, "my-result#ok(value)");
    }

    #[test]
    fn test_variant_arity_code_actions_generates_quickfix() {
        let uri = Url::parse("file:///tmp/test.kettu").expect("valid uri");
        let content = "let v = switch#on(1);";
        let range = Range {
            start: Position::new(0, 8),
            end: Position::new(0, 20),
        };

        let diagnostic = Diagnostic {
            range,
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("kettu-checker".to_string()),
            message: "Case 'switch#on' does not accept a payload".to_string(),
            ..Default::default()
        };

        let actions = variant_arity_code_actions(&uri, content, &[diagnostic]);
        assert_eq!(actions.len(), 1, "should produce one quick fix");

        let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
            panic!("expected code action");
        };
        assert_eq!(action.title, "Remove payload argument");
        let edit = action.edit.as_ref().expect("workspace edit");
        let file_edits = edit
            .changes
            .as_ref()
            .and_then(|c| c.get(&uri))
            .expect("uri edits");
        assert_eq!(file_edits[0].new_text, "switch#on");
    }
}
