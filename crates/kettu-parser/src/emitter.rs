//! WIT Emitter
//!
//! Converts a Kettu AST back to pure WIT format, stripping Kettu-specific
//! extensions like function bodies.
//!
//! The emitter also handles **monomorphization** of generic types:
//! - Collects all `Ty::Generic` instantiations (e.g., `pair<s32>`)
//! - Generates concrete type definitions (e.g., `pair-s32`)
//! - Replaces generic type references with monomorphized names

use crate::ast::*;
use std::collections::{HashMap, HashSet};

/// Emit a WitFile as pure WIT syntax.
pub fn emit_wit(file: &WitFile) -> String {
    let mut output = String::new();
    let mut emitter = WitEmitter::new(&mut output);
    emitter.emit_file(file);
    output
}

struct WitEmitter<'a> {
    output: &'a mut String,
    indent: usize,
    /// Maps monomorphized name -> (generic_name, type_args) for substitution
    /// e.g., "pair-s32" -> ("pair", [s32])
    monomorphized: HashMap<String, (String, Vec<Ty>)>,
    /// Generic type definitions we've seen, keyed by name
    /// Maps generic name -> type_params and kind (fields/cases)
    generic_defs: HashMap<String, GenericTypeDef>,
    /// Set of monomorphized types already emitted
    emitted_monomorphs: HashSet<String>,
}

/// Stores info about a generic type definition for later monomorphization
#[derive(Clone)]
struct GenericTypeDef {
    type_params: Vec<String>,
    kind: GenericTypeDefKind,
}

#[derive(Clone)]
enum GenericTypeDefKind {
    Record { fields: Vec<(String, Ty)> },
    Variant { cases: Vec<(String, Option<Ty>)> },
    Alias { target: Ty },
}

impl<'a> WitEmitter<'a> {
    fn new(output: &'a mut String) -> Self {
        Self {
            output,
            indent: 0,
            monomorphized: HashMap::new(),
            generic_defs: HashMap::new(),
            emitted_monomorphs: HashSet::new(),
        }
    }

    /// Generate a monomorphized type name from a generic name and type arguments
    fn mangle_name(&self, name: &str, args: &[Ty]) -> String {
        let mut result = name.to_string();
        for arg in args {
            result.push('-');
            result.push_str(&self.ty_to_name_part(arg));
        }
        result
    }

    /// Convert a type to a name fragment for mangling
    fn ty_to_name_part(&self, ty: &Ty) -> String {
        match ty {
            Ty::Primitive(p, _) => match p {
                PrimitiveTy::U8 => "u8".to_string(),
                PrimitiveTy::U16 => "u16".to_string(),
                PrimitiveTy::U32 => "u32".to_string(),
                PrimitiveTy::U64 => "u64".to_string(),
                PrimitiveTy::S8 => "s8".to_string(),
                PrimitiveTy::S16 => "s16".to_string(),
                PrimitiveTy::S32 => "s32".to_string(),
                PrimitiveTy::S64 => "s64".to_string(),
                PrimitiveTy::F32 => "f32".to_string(),
                PrimitiveTy::F64 => "f64".to_string(),
                PrimitiveTy::Bool => "bool".to_string(),
                PrimitiveTy::Char => "char".to_string(),
                PrimitiveTy::String => "string".to_string(),
            },
            Ty::Named(id) => id.name.clone(),
            Ty::List { element, .. } => format!("list-{}", self.ty_to_name_part(element)),
            Ty::Option { inner, .. } => format!("option-{}", self.ty_to_name_part(inner)),
            Ty::Generic { name, args, .. } => self.mangle_name(&name.name, args),
            _ => "unknown".to_string(),
        }
    }

    /// Get the monomorphized name for a generic instantiation, registering it if needed
    fn get_or_register_monomorph(&mut self, name: &str, args: &[Ty]) -> String {
        let key = self.mangle_name(name, args);
        if !self.monomorphized.contains_key(&key) {
            // Store the generic name and type arguments for later substitution
            self.monomorphized
                .insert(key.clone(), (name.to_string(), args.to_vec()));
        }
        key
    }

    fn emit_file(&mut self, file: &WitFile) {
        // Package declaration
        if let Some(pkg) = &file.package {
            self.emit_package(pkg);
            self.newline();
        }

        // Top-level items
        for (i, item) in file.items.iter().enumerate() {
            if i > 0 {
                self.newline();
            }
            self.emit_top_level_item(item);
        }
    }

    fn emit_package(&mut self, pkg: &PackageDecl) {
        self.write("package ");

        // namespace:name format
        let namespace: Vec<_> = pkg.path.namespace.iter().map(|i| i.name.as_str()).collect();
        let name: Vec<_> = pkg.path.name.iter().map(|i| i.name.as_str()).collect();

        if !namespace.is_empty() {
            self.write(&namespace.join(":"));
            self.write(":");
        }
        self.write(&name.join("/"));

        if let Some(version) = &pkg.path.version {
            self.write("@");
            self.emit_version(version);
        }

        self.write(";");
        self.newline();
    }

    fn emit_top_level_item(&mut self, item: &TopLevelItem) {
        self.write_indent();
        match item {
            TopLevelItem::Interface(iface) => self.emit_interface(iface),
            TopLevelItem::World(world) => self.emit_world(world),
            TopLevelItem::Use(use_stmt) => self.emit_top_level_use(use_stmt),
            TopLevelItem::NestedPackage(pkg) => self.emit_nested_package(pkg),
        }
    }

    fn emit_nested_package(&mut self, pkg: &NestedPackage) {
        self.write("package ");

        // namespace:name format
        let namespace: Vec<_> = pkg.path.namespace.iter().map(|i| i.name.as_str()).collect();
        let name: Vec<_> = pkg.path.name.iter().map(|i| i.name.as_str()).collect();

        if !namespace.is_empty() {
            self.write(&namespace.join(":"));
            self.write(":");
        }
        self.write(&name.join("/"));

        if let Some(version) = &pkg.path.version {
            self.write("@");
            self.emit_version(version);
        }

        self.write(" {");
        self.newline();
        self.indent += 1;

        for (i, item) in pkg.items.iter().enumerate() {
            if i > 0 {
                self.newline();
            }
            self.emit_top_level_item(item);
        }

        self.indent -= 1;
        self.write_indent();
        self.write("}");
        self.newline();
    }

    fn emit_interface(&mut self, iface: &Interface) {
        // Feature gates
        for gate in &iface.gates {
            self.emit_gate(gate);
        }

        self.write("interface ");
        self.write(&iface.name.name);
        self.write(" {");
        self.newline();
        self.indent += 1;

        for item in &iface.items {
            self.emit_interface_item(item);
        }

        // Emit monomorphized types based on collected generic instantiations
        self.emit_monomorphized_types();

        self.indent -= 1;
        self.write_indent();
        self.write("}");
        self.newline();
    }

    /// Emit concrete type definitions for all registered generic instantiations
    fn emit_monomorphized_types(&mut self) {
        // Collect entries to emit (to avoid borrowing issues)
        let mono_entries: Vec<(String, String, Vec<Ty>)> = self
            .monomorphized
            .iter()
            .map(|(k, (name, args))| (k.clone(), name.clone(), args.clone()))
            .collect();

        for (mono_name, generic_name, type_args) in mono_entries {
            // Skip if already emitted
            if self.emitted_monomorphs.contains(&mono_name) {
                continue;
            }
            self.emitted_monomorphs.insert(mono_name.clone());

            if let Some(def) = self.generic_defs.get(&generic_name).cloned() {
                // Build substitution map: type_param -> concrete_type
                let subst: HashMap<String, Ty> = def
                    .type_params
                    .iter()
                    .zip(type_args.iter())
                    .map(|(param, ty)| (param.clone(), ty.clone()))
                    .collect();

                self.write_indent();
                self.emit_monomorph_def(&mono_name, &def, &subst);
                self.newline();
            }
        }
    }

    /// Emit a monomorphized type definition with type substitution
    fn emit_monomorph_def(
        &mut self,
        mono_name: &str,
        def: &GenericTypeDef,
        subst: &HashMap<String, Ty>,
    ) {
        match &def.kind {
            GenericTypeDefKind::Alias { target } => {
                self.write("type ");
                self.write(mono_name);
                self.write(" = ");
                self.emit_type_with_subst(target, subst);
                self.write(";");
            }
            GenericTypeDefKind::Record { fields } => {
                self.write("record ");
                self.write(mono_name);
                self.write(" {");
                self.newline();
                self.indent += 1;

                for (field_name, field_ty) in fields {
                    self.write_indent();
                    self.write(field_name);
                    self.write(": ");
                    self.emit_type_with_subst(field_ty, subst);
                    self.write(",");
                    self.newline();
                }

                self.indent -= 1;
                self.write_indent();
                self.write("}");
            }
            GenericTypeDefKind::Variant { cases } => {
                self.write("variant ");
                self.write(mono_name);
                self.write(" {");
                self.newline();
                self.indent += 1;

                for (case_name, case_ty) in cases {
                    self.write_indent();
                    self.write(case_name);
                    if let Some(ty) = case_ty {
                        self.write("(");
                        self.emit_type_with_subst(ty, subst);
                        self.write(")");
                    }
                    self.write(",");
                    self.newline();
                }

                self.indent -= 1;
                self.write_indent();
                self.write("}");
            }
        }
    }

    /// Emit a type with substitution of type parameters
    fn emit_type_with_subst(&mut self, ty: &Ty, subst: &HashMap<String, Ty>) {
        match ty {
            Ty::Named(id) if subst.contains_key(&id.name) => {
                // Substitute the type parameter with the concrete type
                self.emit_type(subst.get(&id.name).unwrap());
            }
            _ => self.emit_type(ty),
        }
    }

    fn emit_interface_item(&mut self, item: &InterfaceItem) {
        match item {
            InterfaceItem::Func(func) => {
                // Skip generic function templates - they will be monomorphized at usage
                if !func.type_params.is_empty() {
                    // Store for later use if we need to monomorphize
                    // For now, just skip - generic functions are Kettu-only
                    return;
                }
                self.write_indent();
                self.emit_func(func);
                self.newline();
            }
            InterfaceItem::TypeDef(typedef) => {
                self.write_indent();
                self.emit_typedef(typedef);
                self.newline();
            }
            InterfaceItem::Use(use_stmt) => {
                self.write_indent();
                self.emit_use_statement(use_stmt);
                self.newline();
            }
        }
    }

    fn emit_func(&mut self, func: &Func) {
        // Feature gates
        for gate in &func.gates {
            self.emit_gate(gate);
            self.write(" ");
        }

        self.write(&func.name.name);
        self.write(": ");

        // NOTE: Don't emit `async` - wit-parser doesn't support it yet (Preview 3)
        // The async behavior is handled in the core wasm ABI, not the WIT interface
        // if func.is_async {
        //     self.write("async ");
        // }

        self.write("func(");

        // Parameters
        for (i, param) in func.params.iter().enumerate() {
            if i > 0 {
                self.write(", ");
            }
            self.write(&param.name.name);
            self.write(": ");
            self.emit_type(&param.ty);
        }

        self.write(")");

        // Result
        if let Some(result) = &func.result {
            self.write(" -> ");
            self.emit_type(result);
        }

        // NOTE: We skip the function body - this is the key Kettu->WIT conversion!
        self.write(";");
    }

    fn emit_typedef(&mut self, typedef: &TypeDef) {
        // Feature gates
        for gate in &typedef.gates {
            self.emit_gate(gate);
            self.write(" ");
        }

        // Check if this is a generic type definition - if so, store it for later monomorphization
        match &typedef.kind {
            TypeDefKind::Alias {
                name,
                ty,
                type_params,
            } if !type_params.is_empty() => {
                // Store the generic definition for later monomorphization
                self.generic_defs.insert(
                    name.name.clone(),
                    GenericTypeDef {
                        type_params: type_params.iter().map(|p| p.name.clone()).collect(),
                        kind: GenericTypeDefKind::Alias { target: ty.clone() },
                    },
                );
                return; // Don't emit the generic template
            }
            TypeDefKind::Record {
                name,
                fields,
                type_params,
            } if !type_params.is_empty() => {
                // Store the generic definition for later monomorphization
                self.generic_defs.insert(
                    name.name.clone(),
                    GenericTypeDef {
                        type_params: type_params.iter().map(|p| p.name.clone()).collect(),
                        kind: GenericTypeDefKind::Record {
                            fields: fields
                                .iter()
                                .map(|f| (f.name.name.clone(), f.ty.clone()))
                                .collect(),
                        },
                    },
                );
                return; // Don't emit the generic template
            }
            TypeDefKind::Variant {
                name,
                cases,
                type_params,
            } if !type_params.is_empty() => {
                // Store the generic definition for later monomorphization
                self.generic_defs.insert(
                    name.name.clone(),
                    GenericTypeDef {
                        type_params: type_params.iter().map(|p| p.name.clone()).collect(),
                        kind: GenericTypeDefKind::Variant {
                            cases: cases
                                .iter()
                                .map(|c| (c.name.name.clone(), c.ty.clone()))
                                .collect(),
                        },
                    },
                );
                return; // Don't emit the generic template
            }
            _ => {} // Non-generic types are emitted normally
        }

        match &typedef.kind {
            TypeDefKind::Alias { name, ty, .. } => {
                self.write("type ");
                self.write(&name.name);
                self.write(" = ");
                self.emit_type(ty);
                self.write(";");
            }
            TypeDefKind::Record { name, fields, .. } => {
                self.write("record ");
                self.write(&name.name);
                self.write(" {");
                self.newline();
                self.indent += 1;

                for field in fields {
                    self.write_indent();
                    self.write(&field.name.name);
                    self.write(": ");
                    self.emit_type(&field.ty);
                    self.write(",");
                    self.newline();
                }

                self.indent -= 1;
                self.write_indent();
                self.write("}");
            }
            TypeDefKind::Variant { name, cases, .. } => {
                self.write("variant ");
                self.write(&name.name);
                self.write(" {");
                self.newline();
                self.indent += 1;

                for case in cases {
                    self.write_indent();
                    self.write(&case.name.name);
                    if let Some(ty) = &case.ty {
                        self.write("(");
                        self.emit_type(ty);
                        self.write(")");
                    }
                    self.write(",");
                    self.newline();
                }

                self.indent -= 1;
                self.write_indent();
                self.write("}");
            }
            TypeDefKind::Enum { name, cases } => {
                self.write("enum ");
                self.write(&name.name);
                self.write(" {");
                self.newline();
                self.indent += 1;

                for case in cases {
                    self.write_indent();
                    self.write(&case.name);
                    self.write(",");
                    self.newline();
                }

                self.indent -= 1;
                self.write_indent();
                self.write("}");
            }
            TypeDefKind::Flags { name, flags } => {
                self.write("flags ");
                self.write(&name.name);
                self.write(" {");
                self.newline();
                self.indent += 1;

                for flag in flags {
                    self.write_indent();
                    self.write(&flag.name);
                    self.write(",");
                    self.newline();
                }

                self.indent -= 1;
                self.write_indent();
                self.write("}");
            }
            TypeDefKind::Resource { name, methods } => {
                self.write("resource ");
                self.write(&name.name);

                if methods.is_empty() {
                    self.write(";");
                } else {
                    self.write(" {");
                    self.newline();
                    self.indent += 1;

                    for method in methods {
                        self.write_indent();
                        self.emit_resource_method(method);
                        self.newline();
                    }

                    self.indent -= 1;
                    self.write_indent();
                    self.write("}");
                }
            }
        }
    }

    fn emit_resource_method(&mut self, method: &ResourceMethod) {
        match method {
            ResourceMethod::Constructor { params, .. } => {
                self.write("constructor(");
                for (i, param) in params.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.write(&param.name.name);
                    self.write(": ");
                    self.emit_type(&param.ty);
                }
                self.write(");");
            }
            ResourceMethod::Method(func) => {
                self.write(&func.name.name);
                self.write(": func(");
                for (i, param) in func.params.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.write(&param.name.name);
                    self.write(": ");
                    self.emit_type(&param.ty);
                }
                self.write(")");
                if let Some(result) = &func.result {
                    self.write(" -> ");
                    self.emit_type(result);
                }
                self.write(";");
            }
            ResourceMethod::Static(func) => {
                self.write(&func.name.name);
                self.write(": static func(");
                for (i, param) in func.params.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.write(&param.name.name);
                    self.write(": ");
                    self.emit_type(&param.ty);
                }
                self.write(")");
                if let Some(result) = &func.result {
                    self.write(" -> ");
                    self.emit_type(result);
                }
                self.write(";");
            }
        }
    }

    fn emit_type(&mut self, ty: &Ty) {
        match ty {
            Ty::Primitive(p, _) => {
                let s = match p {
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
                };
                self.write(s);
            }
            Ty::Named(id) => {
                self.write(&id.name);
            }
            Ty::List { element, size, .. } => {
                self.write("list<");
                self.emit_type(element);
                if let Some(size) = size {
                    self.write(&format!(", {}>", size));
                } else {
                    self.write(">");
                }
            }
            Ty::Option { inner, .. } => {
                self.write("option<");
                self.emit_type(inner);
                self.write(">");
            }
            Ty::Result { ok, err, .. } => {
                self.write("result");
                match (ok, err) {
                    (Some(ok), Some(err)) => {
                        self.write("<");
                        self.emit_type(ok);
                        self.write(", ");
                        self.emit_type(err);
                        self.write(">");
                    }
                    (Some(ok), None) => {
                        self.write("<");
                        self.emit_type(ok);
                        self.write(">");
                    }
                    (None, Some(err)) => {
                        self.write("<_, ");
                        self.emit_type(err);
                        self.write(">");
                    }
                    (None, None) => {}
                }
            }
            Ty::Tuple { elements, .. } => {
                self.write("tuple<");
                for (i, elem) in elements.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.emit_type(elem);
                }
                self.write(">");
            }
            Ty::Future { inner, .. } => {
                self.write("future");
                if let Some(inner) = inner {
                    self.write("<");
                    self.emit_type(inner);
                    self.write(">");
                }
            }
            Ty::Stream { inner, .. } => {
                self.write("stream");
                if let Some(inner) = inner {
                    self.write("<");
                    self.emit_type(inner);
                    self.write(">");
                }
            }
            Ty::Borrow { resource, .. } => {
                self.write("borrow<");
                self.write(&resource.name);
                self.write(">");
            }
            Ty::Own { resource, .. } => {
                self.write("own<");
                self.write(&resource.name);
                self.write(">");
            }
            Ty::Generic { name, args, .. } => {
                // Emit the monomorphized name instead of generic syntax
                // e.g., pair<s32> becomes pair-s32
                let mono_name = self.get_or_register_monomorph(&name.name, args);
                self.write(&mono_name);
            }
        }
    }

    fn emit_world(&mut self, world: &World) {
        // Feature gates
        for gate in &world.gates {
            self.emit_gate(gate);
        }

        self.write("world ");
        self.write(&world.name.name);
        self.write(" {");
        self.newline();
        self.indent += 1;

        for item in &world.items {
            self.emit_world_item(item);
        }

        self.indent -= 1;
        self.write_indent();
        self.write("}");
        self.newline();
    }

    fn emit_world_item(&mut self, item: &WorldItem) {
        self.write_indent();

        match item {
            WorldItem::Import(ie) => {
                self.write("import ");
                self.emit_import_export(ie);
                self.newline();
            }
            WorldItem::Export(ie) => {
                self.write("export ");
                self.emit_import_export(ie);
                self.newline();
            }
            WorldItem::TypeDef(typedef) => {
                self.emit_typedef(typedef);
                self.newline();
            }
            WorldItem::Use(use_stmt) => {
                self.emit_use_statement(use_stmt);
                self.newline();
            }
            WorldItem::Include(include) => {
                self.write("include ");
                self.emit_use_path(&include.path);
                self.write(";");
                self.newline();
            }
        }
    }

    fn emit_import_export(&mut self, ie: &ImportExport) {
        if let Some(name) = &ie.name {
            self.write(&name.name);
            self.write(": ");
        }

        match &ie.kind {
            ImportExportKind::Path(path) => {
                self.emit_use_path(path);
                self.write(";");
            }
            ImportExportKind::Func(func) => {
                self.emit_func(func);
            }
            ImportExportKind::Interface(items) => {
                self.write("{");
                self.newline();
                self.indent += 1;

                for item in items {
                    self.emit_interface_item(item);
                }

                self.indent -= 1;
                self.write_indent();
                self.write("}");
            }
        }
    }

    fn emit_use_statement(&mut self, use_stmt: &UseStatement) {
        self.write("use ");
        self.emit_use_path(&use_stmt.path);
        self.write(".{");

        for (i, item) in use_stmt.names.iter().enumerate() {
            if i > 0 {
                self.write(", ");
            }
            self.write(&item.name.name);
            if let Some(alias) = &item.alias {
                self.write(" as ");
                self.write(&alias.name);
            }
        }

        self.write("};");
    }

    fn emit_top_level_use(&mut self, use_stmt: &TopLevelUse) {
        self.write("use ");
        self.emit_use_path(&use_stmt.path);

        if let Some(alias) = &use_stmt.alias {
            self.write(" as ");
            self.write(&alias.name);
        }

        self.write(";");
        self.newline();
    }

    fn emit_use_path(&mut self, path: &UsePath) {
        if let Some(pkg) = &path.package {
            let namespace: Vec<_> = pkg.namespace.iter().map(|i| i.name.as_str()).collect();
            let name: Vec<_> = pkg.name.iter().map(|i| i.name.as_str()).collect();

            if !namespace.is_empty() {
                self.write(&namespace.join(":"));
                self.write(":");
            }
            self.write(&name.join("/"));
            self.write("/");
        }

        self.write(&path.interface.name);
    }

    fn emit_gate(&mut self, gate: &Gate) {
        match gate {
            Gate::Since { version } => {
                self.write("@since(version = ");
                self.emit_version(version);
                self.write(")");
            }
            Gate::Unstable { feature } => {
                self.write("@unstable(feature = ");
                self.write(&feature.name);
                self.write(")");
            }
            Gate::Deprecated { version } => {
                self.write("@deprecated(version = ");
                self.emit_version(version);
                self.write(")");
            }
            Gate::Test => {
                // @test is a Kettu-only attribute, not emitted to WIT
            }
        }
    }

    fn emit_version(&mut self, version: &Version) {
        let ver_str = if let Some(ref pre) = version.prerelease {
            format!(
                "{}.{}.{}-{}",
                version.major, version.minor, version.patch, pre
            )
        } else {
            format!("{}.{}.{}", version.major, version.minor, version.patch)
        };
        self.write(&ver_str);
    }

    fn write(&mut self, s: &str) {
        self.output.push_str(s);
    }

    fn write_indent(&mut self) {
        for _ in 0..self.indent {
            self.output.push_str("    ");
        }
    }

    fn newline(&mut self) {
        self.output.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_file;

    #[test]
    fn test_emit_simple_interface() {
        let source = r#"
            package local:demo;

            interface host {
                log: func(msg: string);
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");

        let output = emit_wit(&ast);

        assert!(output.contains("package local:demo;"));
        assert!(output.contains("interface host"));
        assert!(output.contains("log: func(msg: string)"));
    }

    #[test]
    fn test_emit_strips_function_bodies() {
        let source = r#"
            package local:demo;

            interface host {
                log: func(msg: string) {
                    println(msg)
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");

        let output = emit_wit(&ast);

        // Should have the function signature ending with semicolon (not a body)
        assert!(output.contains("log: func(msg: string);"));
        // Should NOT have the function body content
        assert!(!output.contains("println"));
        // Interface braces are fine, but function body braces should not exist
        // Count braces - should only have 2 (interface open/close)
        assert_eq!(
            output.matches('{').count(),
            1,
            "Should only have interface opening brace"
        );
        assert_eq!(
            output.matches('}').count(),
            1,
            "Should only have interface closing brace"
        );
    }

    #[test]
    fn test_emit_monomorphizes_generics() {
        let source = r#"
            package local:demo;

            interface generics {
                record pair<T> {
                    first: T,
                    second: T,
                }
                
                make-pair: func() -> pair<s32>;
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");

        let output = emit_wit(&ast);

        // Should NOT contain the generic template
        assert!(
            !output.contains("record pair<T>"),
            "Should not emit generic template"
        );

        // Should contain the monomorphized type
        assert!(
            output.contains("pair-s32"),
            "Should emit monomorphized type name"
        );

        // Should have concrete field types, not T
        assert!(
            output.contains("first: s32"),
            "Should substitute T with s32"
        );
        assert!(
            output.contains("second: s32"),
            "Should substitute T with s32"
        );

        // Function should reference monomorphized type
        assert!(
            output.contains("-> pair-s32"),
            "Function should return monomorphized type"
        );
    }

    #[test]
    fn test_emit_monomorphizes_multiple_instantiations() {
        let source = r#"
            package local:demo;

            interface generics {
                record pair<T> {
                    first: T,
                    second: T,
                }
                
                make-int-pair: func() -> pair<s32>;
                make-string-pair: func() -> pair<string>;
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");

        let output = emit_wit(&ast);

        // Should have both monomorphized types
        assert!(output.contains("pair-s32"), "Should emit pair-s32");
        assert!(output.contains("pair-string"), "Should emit pair-string");

        // Each should have correct field types
        assert!(
            output.contains("first: s32"),
            "pair-s32 should have s32 fields"
        );
        assert!(
            output.contains("first: string"),
            "pair-string should have string fields"
        );
    }

    #[test]
    fn test_emit_skips_generic_functions() {
        let source = r#"
            package local:demo;

            interface generics {
                // Generic function - should NOT be emitted
                swap<T>: func(a: T, b: T) -> tuple<T, T>;
                
                // Non-generic function - should be emitted
                add: func(a: s32, b: s32) -> s32;
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");

        let output = emit_wit(&ast);

        // Generic function should NOT be in output
        assert!(
            !output.contains("swap<T>"),
            "Should not emit generic function"
        );
        assert!(!output.contains("swap:"), "Should not emit swap function");

        // Non-generic function SHOULD be in output
        assert!(
            output.contains("add: func"),
            "Should emit non-generic function"
        );
    }

    #[test]
    fn test_emit_nested_package() {
        // We bypass the parser here because kettu-parser's grammar might not support
        // parsing nested packages yet, but we are testing AST emission.
        let mut ast = WitFile {
            package: Some(PackageDecl {
                path: PackagePath {
                    namespace: vec![Id::new("local", 0..0)],
                    name: vec![Id::new("demo", 0..0)],
                    version: None,
                },
                span: 0..0,
            }),
            items: vec![],
        };

        let nested = NestedPackage {
            path: PackagePath {
                namespace: vec![Id::new("local", 0..0)],
                name: vec![Id::new("nested", 0..0)],
                version: Some(Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                    prerelease: None,
                    span: 0..0,
                }),
            },
            items: vec![TopLevelItem::Interface(Interface {
                gates: vec![],
                name: Id::new("my-iface", 0..0),
                items: vec![InterfaceItem::Func(Func {
                    gates: vec![],
                    name: Id::new("run", 0..0),
                    type_params: vec![],
                    is_async: false,
                    params: vec![],
                    result: None,
                    body: None,
                    span: 0..0,
                })],
                span: 0..0,
            })],
            span: 0..0,
        };

        ast.items.push(TopLevelItem::NestedPackage(nested));

        let output = emit_wit(&ast);
        println!("Output: {}", output);

        assert!(output.contains("package local:demo;"));
        assert!(output.contains("package local:nested@1.0.0 {"));
        assert!(output.contains("    interface my-iface {"));
        assert!(output.contains("        run: func();"));
        assert!(output.contains("    }"));
        assert!(output.contains("}"));
    }
}
