//! Core WASM Module Compiler
//!
//! Compiles Kettu AST (with function bodies) into a core WASM module.

use kettu_parser::{
    BinOp, Expr, Func, FuncBody, InterfaceItem, Param, Pattern, PrimitiveTy, ResourceMethod,
    SimdLane, SimdOp, Statement, StringPart, TopLevelItem, Ty, TypeDef, TypeDefKind, WitFile,
};
use std::collections::HashMap;
use std::borrow::Cow;
use std::ops::Range;
use wasm_encoder::{
    CodeSection, CustomSection, DataSection, ElementSection, Elements, EntityType, ExportKind,
    ExportSection, Function, FunctionSection, GlobalSection, GlobalType, ImportSection,
    Instruction, MemorySection, MemoryType, Module, NameMap, NameSection, RefType, TableSection,
    TableType, TypeSection, ValType,
};

/// Type information for a record, storing field names and their offsets
#[derive(Debug, Clone)]
struct RecordTypeInfo {
    /// Field names to offsets (in bytes)
    fields: Vec<(String, usize)>,
}

impl RecordTypeInfo {
    fn from_fields(field_names: &[(String, usize)]) -> Self {
        Self {
            fields: field_names.to_vec(),
        }
    }

    fn get_offset(&self, field_name: &str) -> Option<usize> {
        self.fields
            .iter()
            .find(|(name, _)| name == field_name)
            .map(|(_, offset)| *offset)
    }
}

/// Compilation options
#[derive(Debug, Clone, Default)]
pub struct CompileOptions {
    /// If true, only produce a core module (no component wrapping)
    pub core_only: bool,
    /// Memory pages (64KB each)
    pub memory_pages: u32,
    /// If true, enable WASI Preview 3 async ABI (experimental)
    /// - Async functions return i32 status code instead of result
    /// - Results passed via task.return canonical built-in
    pub wasip3: bool,
    /// If true, enable shared memory and thread-spawn support
    pub threads: bool,
    /// If true, emit DWARF debug sections (for DAP/source-level debugging)
    pub emit_dwarf: bool,
    /// If true, keep function names via the name section
    pub keep_names: bool,
    /// Optional source text used for line mapping in debug sections
    pub debug_source: Option<String>,
}

/// Compilation error
#[derive(Debug, Clone)]
pub struct CompileError {
    pub message: String,
    pub span: Option<std::ops::Range<usize>>,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CompileError {}

/// Compile a Kettu file to a core WASM module
pub fn compile_module(file: &WitFile, options: &CompileOptions) -> Result<Vec<u8>, CompileError> {
    let mut compiler = ModuleCompiler::new(options);
    compiler.compile(file)
}

/// Compile a Kettu file with imported dependencies
///
/// `imports` is a list of (interface_alias, imported_file) pairs.
/// Functions from imported interfaces can be called as `alias.function()`.
pub fn compile_module_with_imports(
    file: &WitFile,
    imports: &[(String, &WitFile)],
    options: &CompileOptions,
) -> Result<Vec<u8>, CompileError> {
    let mut compiler = ModuleCompiler::new(options);

    // First, collect function definitions from all imported files
    for (alias, imported_file) in imports {
        compiler.register_imported_interface(alias, imported_file)?;
    }

    // Then compile the main file
    compiler.compile(file)
}

fn offset_to_line(source: &str, offset: usize) -> usize {
    source[..offset.min(source.len())]
        .bytes()
        .filter(|b| *b == b'\n')
        .count()
        + 1
}

struct ModuleCompiler<'a> {
    options: &'a CompileOptions,
    /// Type index -> (params, results)
    types: Vec<(Vec<ValType>, Vec<ValType>)>,
    /// Function name -> (type_idx, func_idx, is_import)
    functions: HashMap<String, (u32, u32, bool)>,
    /// Import count (imports come before local functions)
    import_count: u32,
    /// Local function count
    local_func_count: u32,
    /// Exported functions
    exports: Vec<(String, u32)>,
    /// Source spans for functions (func_idx -> span)
    func_spans: Vec<(u32, Range<usize>)>,
    /// Function bodies to compile
    func_bodies: Vec<(String, Func)>,
    /// String literal data: (offset, bytes)
    string_data: Vec<(u32, Vec<u8>)>,
    /// Next available offset for string data
    string_offset: u32,
    /// Index of the built-in alloc function (if emitted)
    alloc_func_idx: Option<u32>,
    /// Index of the built-in string concat function (if emitted)
    str_concat_func_idx: Option<u32>,
    /// Index of the built-in string equality function (if emitted)
    str_eq_func_idx: Option<u32>,
    /// Index of the built-in arena reset function (if emitted)
    arena_reset_func_idx: Option<u32>,
    /// Lambda functions: (type_idx, func_idx, captures, params, body) for each lambda emitted
    lambda_bodies: Vec<(u32, u32, Vec<kettu_parser::Id>, Vec<kettu_parser::Id>, Expr)>,
    /// Counter for unique lambda names
    next_lambda_id: u32,
    /// Closure info: variable name -> capture names (for callable closures)
    closure_info: HashMap<String, Vec<String>>,
    /// Imported interfaces: interface_alias -> (interface_name, HashMap<func_name, func_idx>)
    imported_interfaces: HashMap<String, (String, HashMap<String, u32>)>,
    /// Index of the task.return canonical built-in (for async exports)
    task_return_func_idx: Option<u32>,
    /// Type index for task.return for each result type
    task_return_types: HashMap<Vec<ValType>, u32>,
    /// Callback function bodies for async functions: (entry_func_name, func, state_local_count)
    callback_bodies: Vec<(String, Func, u32)>,
    /// Index of waitable-set.new import (for async state machine)
    waitable_set_new_idx: Option<u32>,
    /// Index of waitable-set.wait import
    waitable_set_wait_idx: Option<u32>,
    /// Index of subtask.drop import
    subtask_drop_idx: Option<u32>,
    /// Next state memory offset (for saving async state between callbacks)
    async_state_offset: u32,
    spawn_bodies: Vec<(u32, u32, Vec<Statement>)>,
    thread_spawn_idx: Option<u32>,
    next_spawn_id: u32,
    /// Tracks which variables are shared memory handles (for atomic block desugaring)
    shared_locals: std::collections::HashSet<String>,
    /// Break target depth: how many wasm blocks to `br` past to reach the break target.
    /// Set to Some when inside a while/for loop body.
    loop_break_depth: Option<u32>,
    /// Continue target depth: how many wasm blocks to `br` past to reach the continue target.
    loop_continue_depth: Option<u32>,
}

impl<'a> ModuleCompiler<'a> {
    fn new(options: &'a CompileOptions) -> Self {
        Self {
            options,
            types: Vec::new(),
            functions: HashMap::new(),
            import_count: 0,
            local_func_count: 0,
            exports: Vec::new(),
            func_spans: Vec::new(),
            func_bodies: Vec::new(),
            string_data: Vec::new(),
            string_offset: 0,
            alloc_func_idx: None,
            str_concat_func_idx: None,
            str_eq_func_idx: None,
            arena_reset_func_idx: None,
            lambda_bodies: Vec::new(),
            next_lambda_id: 0,
            closure_info: HashMap::new(),
            imported_interfaces: HashMap::new(),
            task_return_func_idx: None,
            task_return_types: HashMap::new(),
            callback_bodies: Vec::new(),
            waitable_set_new_idx: None,
            waitable_set_wait_idx: None,
            subtask_drop_idx: None,
            async_state_offset: 0,
            spawn_bodies: Vec::new(),
            thread_spawn_idx: None,
            next_spawn_id: 0,
            shared_locals: std::collections::HashSet::new(),
            loop_break_depth: None,
            loop_continue_depth: None,
        }
    }

    fn emit_name_section(&self, module: &mut Module) {
        if self.exports.is_empty() {
            return;
        }

        let mut func_map = NameMap::new();
        for (export, idx) in &self.exports {
            func_map.append(*idx, export);
        }

        let mut names = NameSection::new();
        names.functions(&func_map);
        module.section(&names);
    }

    fn emit_debug_sections(&self, module: &mut Module) {
        use std::collections::HashMap;

        let source = self.options.debug_source.as_deref();
        let mut idx_to_export: HashMap<u32, &str> = HashMap::new();
        for (name, idx) in &self.exports {
            idx_to_export.insert(*idx, name);
        }

        let mut entries = Vec::new();
        // Custom debug payload format:
        //   kettu-dwarf\n
        //   <func_idx>:<export_name_or_placeholder>:<line>:<byte_offset>\n...
        // This favors readability and keeps enough information for DAP consumers
        // to correlate exports with spans without introducing a full DWARF encoder.
        for (idx, span) in &self.func_spans {
            let line = source.map(|s| offset_to_line(s, span.start)).unwrap_or(1);
            let name = idx_to_export
                .get(idx)
                .copied()
                .unwrap_or("<unnamed>");
            entries.push(format!(
                "{}:{}:{}:{}",
                idx, name, line, span.start
            ));
        }

        let info_payload = format!("kettu-dwarf\n{}", entries.join("\n"));
        let info_section = CustomSection {
            name: ".debug_info".into(),
            data: Cow::from(info_payload.into_bytes()),
        };
        module.section(&info_section);

        let line_payload = if let Some(src) = source {
            let mut offsets = Vec::new();
            let mut pos = 0usize;
            offsets.push(0);
            for b in src.as_bytes() {
                if *b == b'\n' {
                    offsets.push(pos + 1);
                }
                pos += 1;
            }
            format!(
                "lines:{}",
                offsets
                    .iter()
                    .map(|o| o.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            )
        } else {
            "lines:unknown".to_string()
        };
        let line_section = CustomSection {
            name: ".debug_line".into(),
            data: Cow::from(line_payload.into_bytes()),
        };
        module.section(&line_section);
    }
    /// Register an imported interface's functions for qualified calls
    fn register_imported_interface(
        &mut self,
        alias: &str,
        imported_file: &WitFile,
    ) -> Result<(), CompileError> {
        use kettu_parser::{Id, InterfaceItem, TopLevelItem};

        // Build a map of function names to their indices
        let mut func_map: HashMap<String, u32> = HashMap::new();
        let interface_name = alias.to_string();

        // Scan the imported file for interfaces and their functions
        for item in &imported_file.items {
            if let TopLevelItem::Interface(iface) = item {
                for iface_item in &iface.items {
                    if let InterfaceItem::Func(func) = iface_item {
                        // Only include functions with bodies (exported implementations)
                        if func.body.is_some() {
                            // Register function type
                            let type_idx = self.add_func_type(func)?;
                            let func_idx = self.import_count + self.local_func_count;

                            // Create a qualified name for lookup
                            let qualified_name = format!("{}.{}", alias, func.name.name);

                            // Add to functions map with qualified name
                            self.functions
                                .insert(qualified_name.clone(), (type_idx, func_idx, false));

                            // Add to interface function map
                            func_map.insert(func.name.name.clone(), func_idx);

                            // Create a modified function with qualified name for func_bodies
                            let mut qualified_func = func.clone();
                            qualified_func.name = Id {
                                name: qualified_name.clone(),
                                span: func.name.span.clone(),
                            };

                            // Add function body to compile - also add with original name
                            self.func_bodies.push((alias.to_string(), qualified_func));
                            self.functions
                                .insert(func.name.name.clone(), (type_idx, func_idx, false));
                            self.func_spans.push((func_idx, func.span.clone()));
                            self.local_func_count += 1;
                        }
                    }
                }
            }
        }

        // Store the interface mapping
        self.imported_interfaces
            .insert(alias.to_string(), (interface_name, func_map));

        Ok(())
    }

    /// Pre-register task.return imports for async functions when wasip3 is enabled.
    /// This must be called BEFORE collect_definitions so import_count is correct.
    fn preregister_async_imports(&mut self, file: &WitFile) -> Result<(), CompileError> {
        if !self.options.wasip3 {
            return Ok(());
        }

        // Scan for all async functions and register their task.return signatures
        for item in &file.items {
            if let TopLevelItem::Interface(iface) = item {
                for iface_item in &iface.items {
                    if let InterfaceItem::Func(func) = iface_item {
                        if func.is_async && func.body.is_some() {
                            // Pre-register task.return for this result type
                            if let Some(ref result_ty) = func.result {
                                let result_valtype = self.ty_to_valtype(result_ty)?;
                                self.ensure_task_return_import(&[result_valtype]);
                            } else {
                                // No-result async function: still needs task.return()
                                self.ensure_task_return_import(&[]);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn compile(&mut self, file: &WitFile) -> Result<Vec<u8>, CompileError> {
        // Phase 0: Pre-register async imports (must be before collect_definitions)
        self.preregister_async_imports(file)?;

        // Phase 1: Collect definitions (function signatures, imports)
        self.collect_definitions(file)?;

        // Phase 2: Ensure builtin functions exist
        self.ensure_alloc_func();
        self.ensure_str_concat_func();
        self.ensure_arena_reset_func();

        // Phase 3: Pre-compile all function bodies (discovers lambdas)
        let bodies = self.func_bodies.clone();
        let mut compiled_funcs: Vec<Function> = Vec::new();
        for (_, func) in &bodies {
            let function = self.compile_function(func)?;
            compiled_funcs.push(function);
        }
        let initial_heap_offset = self.string_offset;

        // Phase 4: Compile lambda bodies
        let mut compiled_lambdas: Vec<Function> = Vec::new();
        let mut all_lambda_bodies = Vec::new();

        // Loop while there are lambdas to compile (handling nested lambdas discovered during compilation)
        while !self.lambda_bodies.is_empty() {
            let current_lambdas = std::mem::take(&mut self.lambda_bodies);

            for lambda in current_lambdas {
                let (_, _, captures, params, body) = &lambda;
                let mut locals = HashMap::new();
                // Captures come first as hidden parameters
                for (i, capture) in captures.iter().enumerate() {
                    locals.insert(capture.name.clone(), i as u32);
                }
                // Then regular parameters
                let capture_count = captures.len();
                for (i, param) in params.iter().enumerate() {
                    locals.insert(param.name.clone(), (capture_count + i) as u32);
                }
                let locals_types: HashMap<String, RecordTypeInfo> = HashMap::new();
                let mut func = Function::new(vec![]);
                self.compile_expr_with_locals(&mut func, body, &locals, &locals_types)?;
                func.instruction(&Instruction::End);
                compiled_lambdas.push(func);

                all_lambda_bodies.push(lambda);
            }
        }

        // Restore all lambda bodies for later phases (e.g., TypeSection, FunctionSection)
        self.lambda_bodies = all_lambda_bodies;
        let has_lambdas = !compiled_lambdas.is_empty();

        // Phase 4b: Compile async callback bodies
        let callback_bodies_clone = self.callback_bodies.clone();
        let mut compiled_callbacks: Vec<Function> = Vec::new();
        for (func_name, _original_func, num_locals) in &callback_bodies_clone {
            // Callback signature: (event: i32, p1: i32, p2: i32) -> i32
            // For now, callbacks just return DONE status
            // Full implementation would restore state and resume execution
            let _ = func_name; // suppress unused warning

            // Create callback with 3 params + temp locals
            let local_types: Vec<_> = (0..*num_locals).map(|_| (1, ValType::I32)).collect();
            let mut callback = Function::new(local_types);

            // For MVP: just return status 0 (DONE)
            // A full implementation would:
            // 1. Check event type
            // 2. Restore locals from memory
            // 3. Jump to resume point based on state_id
            // 4. Continue execution
            callback.instruction(&Instruction::I32Const(0)); // status: DONE
            callback.instruction(&Instruction::End);
            compiled_callbacks.push(callback);
        }
        let _has_callbacks = !compiled_callbacks.is_empty();

        // Phase 4c: Compile spawn bodies
        let spawn_bodies_clone = std::mem::take(&mut self.spawn_bodies);
        let mut compiled_spawns: Vec<Function> = Vec::new();
        for (_, _, body_stmts) in &spawn_bodies_clone {
            let locals_types: HashMap<String, RecordTypeInfo> = HashMap::new();
            let locals: HashMap<String, u32> = HashMap::new();
            let mut func = Function::new(vec![]);
            for stmt in body_stmts {
                match stmt {
                    Statement::Expr(e) => {
                        self.compile_expr_with_locals(&mut func, e, &locals, &locals_types)?;
                        func.instruction(&Instruction::Drop);
                    }
                    Statement::Let { name: _, value, .. } => {
                        self.compile_expr_with_locals(&mut func, value, &locals, &locals_types)?;
                        func.instruction(&Instruction::Drop);
                    }
                    _ => {}
                }
            }
            func.instruction(&Instruction::End);
            compiled_spawns.push(func);
        }
        self.spawn_bodies = spawn_bodies_clone;
        let has_spawns = !compiled_spawns.is_empty();

        // Phase 5: Emit WASM sections in correct order
        let mut module = Module::new();

        // 1. Type section (now includes lambda types)
        let mut types = TypeSection::new();
        for (params, results) in &self.types {
            types
                .ty()
                .function(params.iter().copied(), results.iter().copied());
        }
        module.section(&types);

        // 2. Import section
        let mut imports = ImportSection::new();

        // Add task.return imports for async functions when --wasip3 is enabled
        if self.options.wasip3 {
            // Get package path for fully qualified interface names
            let async_interface_name = file
                .package
                .as_ref()
                .map(|p| {
                    let namespace = p
                        .path
                        .namespace
                        .iter()
                        .map(|id| id.name.as_str())
                        .collect::<Vec<_>>()
                        .join(":");
                    let name = p
                        .path
                        .name
                        .iter()
                        .map(|id| id.name.as_str())
                        .collect::<Vec<_>>()
                        .join("/");
                    format!("{}:{}/canon-async", namespace, name)
                })
                .unwrap_or_else(|| "canon-async".to_string());

            let task_return_entries: Vec<_> = self.task_return_types.keys().cloned().collect();
            for result_types in task_return_entries {
                let type_idx = self.get_or_create_type(&result_types, &[]);
                // Import from fully-qualified canon-async interface
                imports.import(
                    &async_interface_name,
                    "task-return",
                    EntityType::Function(type_idx),
                );
            }

            // Add waitable-set and subtask imports if they were used
            if self.waitable_set_new_idx.is_some() {
                let type_idx = self.get_or_create_type(&[], &[ValType::I32]);
                imports.import(
                    &async_interface_name,
                    "waitable-set-new",
                    EntityType::Function(type_idx),
                );
            }
            if self.waitable_set_wait_idx.is_some() {
                let type_idx =
                    self.get_or_create_type(&[ValType::I32, ValType::I32], &[ValType::I32]);
                imports.import(
                    &async_interface_name,
                    "waitable-set-wait",
                    EntityType::Function(type_idx),
                );
            }
            if self.subtask_drop_idx.is_some() {
                let type_idx = self.get_or_create_type(&[ValType::I32], &[]);
                imports.import(
                    &async_interface_name,
                    "subtask-drop",
                    EntityType::Function(type_idx),
                );
            }
        }

        // Add interface function imports
        for item in &file.items {
            if let TopLevelItem::Interface(iface) = item {
                for iface_item in &iface.items {
                    if let InterfaceItem::Func(func) = iface_item {
                        if func.body.is_none() {
                            if let Some(&(type_idx, _, true)) = self.functions.get(&func.name.name)
                            {
                                imports.import(
                                    &iface.name.name,
                                    &func.name.name,
                                    EntityType::Function(type_idx),
                                );
                            }
                        }
                    }
                }
            }
        }

        // Add thread_spawn import when spawn expressions are used
        if self.thread_spawn_idx.is_some() {
            let type_idx = self.get_or_create_type(&[ValType::I32], &[ValType::I32]);
            imports.import("wasi", "thread-spawn", EntityType::Function(type_idx));
        }

        if self.import_count > 0 {
            module.section(&imports);
        }

        // 3. Function section (type indices for local functions + builtins + lambdas)
        // Order: user funcs, builtins (alloc, str_concat, arena_reset), then lambdas
        let mut funcs = FunctionSection::new();
        for (_, func) in &self.func_bodies {
            if let Some(&(type_idx, _, false)) = self.functions.get(&func.name.name) {
                funcs.function(type_idx);
            }
        }
        // Add alloc function
        if let Some(&(type_idx, _, false)) = self.functions.get("$alloc") {
            funcs.function(type_idx);
        }
        // Add str_concat function
        if let Some(&(type_idx, _, false)) = self.functions.get("$str_concat") {
            funcs.function(type_idx);
        }
        // Add str_eq function
        if let Some(&(type_idx, _, false)) = self.functions.get("$str_eq") {
            funcs.function(type_idx);
        }
        // Add arena_reset function
        if let Some(&(type_idx, _, false)) = self.functions.get("$arena_reset") {
            funcs.function(type_idx);
        }
        // Add lambda function type indices (after builtins)
        for (type_idx, _, _, _, _) in &self.lambda_bodies {
            funcs.function(*type_idx);
        }
        // Add async callback function type indices (after lambdas)
        // Callback signature: (event: i32, p1: i32, p2: i32) -> i32
        let callback_type_idx =
            self.get_or_create_type(&[ValType::I32, ValType::I32, ValType::I32], &[ValType::I32]);
        for _ in &callback_bodies_clone {
            funcs.function(callback_type_idx);
        }
        // Spawn body function types
        let spawn_void_t = self.get_or_create_type(&[], &[]);
        for _ in &self.spawn_bodies {
            funcs.function(spawn_void_t);
        }
        if has_spawns {
            let wts_t = self.get_or_create_type(&[ValType::I32, ValType::I32], &[]);
            funcs.function(wts_t);
        }
        if self.local_func_count > 0 || has_lambdas || !callback_bodies_clone.is_empty() || has_spawns {
            module.section(&funcs);
        }

        // 4. Table section (for function references)
        if has_lambdas {
            let mut tables = TableSection::new();
            let table_size = self.lambda_bodies.len() as u64 + 1;
            tables.table(TableType {
                element_type: RefType::FUNCREF,
                table64: false,
                minimum: table_size,
                maximum: Some(table_size),
                shared: false,
            });
            module.section(&tables);
        }

        // 5. Memory section
        let mut memories = MemorySection::new();
        memories.memory(MemoryType {
            minimum: self.options.memory_pages.max(1) as u64,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&memories);

        // 6. Global section
        let mut globals = GlobalSection::new();
        globals.global(
            GlobalType {
                val_type: ValType::I32,
                mutable: true,
                shared: false,
            },
            &wasm_encoder::ConstExpr::i32_const(self.string_offset as i32),
        );
        module.section(&globals);

        // 7. Export section
        let mut exports = ExportSection::new();
        for (name, func_idx) in &self.exports {
            exports.export(name, ExportKind::Func, *func_idx);
        }
        exports.export("memory", ExportKind::Memory, 0);
        if has_spawns {
            let wts_idx = self.import_count
                + self.local_func_count
                + self.lambda_bodies.len() as u32
                + callback_bodies_clone.len() as u32
                + self.spawn_bodies.len() as u32;
            exports.export("wasi_thread_start", ExportKind::Func, wts_idx);
        }
        module.section(&exports);

        // 8. Element section (populate function table)
        if has_lambdas {
            let mut elements = ElementSection::new();
            let func_indices: Vec<u32> = self
                .lambda_bodies
                .iter()
                .map(|(_, func_idx, _, _, _)| *func_idx)
                .collect();
            elements.active(
                Some(0),
                &wasm_encoder::ConstExpr::i32_const(1),
                Elements::Functions(std::borrow::Cow::Borrowed(&func_indices)),
            );
            module.section(&elements);
        }

        // 9. Code section (must match function section order: user funcs, builtins, lambdas)
        let mut code = CodeSection::new();
        for func in compiled_funcs {
            code.function(&func);
        }
        if self.alloc_func_idx.is_some() {
            code.function(&self.build_alloc_function());
        }
        if self.str_concat_func_idx.is_some() {
            code.function(&self.build_str_concat_function());
        }
        if self.str_eq_func_idx.is_some() {
            code.function(&self.build_str_eq_function());
        }
        if self.arena_reset_func_idx.is_some() {
            code.function(&self.build_arena_reset_function(initial_heap_offset));
        }
        // Lambdas come after builtins
        for func in compiled_lambdas {
            code.function(&func);
        }
        // Async callbacks come after lambdas
        for func in compiled_callbacks {
            code.function(&func);
        }
        if self.local_func_count > 0 || has_lambdas || !callback_bodies_clone.is_empty() {
            module.section(&code);
        }

        // 10. Data section
        if !self.string_data.is_empty() {
            let mut data = DataSection::new();
            for (offset, bytes) in &self.string_data {
                data.active(
                    0,
                    &wasm_encoder::ConstExpr::i32_const(*offset as i32),
                    bytes.iter().copied(),
                );
            }
            module.section(&data);
        }

        // 11. Debug metadata
        if self.options.keep_names || self.options.emit_dwarf {
            self.emit_name_section(&mut module);
        }
        if self.options.emit_dwarf {
            self.emit_debug_sections(&mut module);
        }

        Ok(module.finish())
    }

    fn collect_definitions(&mut self, file: &WitFile) -> Result<(), CompileError> {
        // Extract package path for export naming (e.g., "example:simple")
        let package_path = file
            .package
            .as_ref()
            .map(|p| {
                let namespace = p
                    .path
                    .namespace
                    .iter()
                    .map(|id| id.name.as_str())
                    .collect::<Vec<_>>()
                    .join(":");
                let name = p
                    .path
                    .name
                    .iter()
                    .map(|id| id.name.as_str())
                    .collect::<Vec<_>>()
                    .join(":");
                format!("{}:{}", namespace, name)
            })
            .unwrap_or_else(|| "local:component".to_string());

        for item in &file.items {
            if let TopLevelItem::Interface(iface) = item {
                for iface_item in &iface.items {
                    if let InterfaceItem::Func(func) = iface_item {
                        let type_idx = self.add_func_type(func)?;

                        if func.body.is_some() {
                            // Local function (exported)
                            let func_idx = self.import_count + self.local_func_count;
                            self.functions
                                .insert(func.name.name.clone(), (type_idx, func_idx, false));
                            // Component Model export naming: package:namespace/interface#function
                            let export_name =
                                format!("{}/{}#{}", package_path, iface.name.name, func.name.name);
                            self.exports.push((export_name, func_idx));
                            self.func_bodies
                                .push((iface.name.name.clone(), func.clone()));
                            self.func_spans.push((func_idx, func.span.clone()));
                            self.local_func_count += 1;
                        } else {
                            // Imported function
                            let func_idx = self.import_count;
                            self.functions
                                .insert(func.name.name.clone(), (type_idx, func_idx, true));
                            self.import_count += 1;
                        }
                    }
                    // Handle resource type definitions
                    if let InterfaceItem::TypeDef(TypeDef {
                        kind: TypeDefKind::Resource { name, methods },
                        ..
                    }) = iface_item
                    {
                        let resource_name = &name.name;
                        for method in methods {
                            match method {
                                ResourceMethod::Constructor {
                                    params,
                                    result: _,
                                    body,
                                    span,
                                } => {
                                    // Constructor: [constructor]resource-name
                                    // Use unique internal name
                                    let internal_name = format!("[constructor]{}", resource_name);
                                    let ctor_name = kettu_parser::Id {
                                        name: internal_name.clone(),
                                        span: span.clone(),
                                    };
                                    let func = Func {
                                        gates: vec![],
                                        name: ctor_name,
                                        type_params: vec![],
                                        is_async: false,
                                        params: params.clone(),
                                        // Constructor implicitly returns own<resource> (i32 handle)
                                        result: Some(Ty::Primitive(PrimitiveTy::S32, span.clone())),
                                        body: body.clone().or_else(|| {
                                            Some(FuncBody {
                                                statements: vec![],
                                                span: span.clone(),
                                            })
                                        }),
                                        span: span.clone(),
                                    };
                                    let type_idx = self.add_func_type(&func)?;
                                    let func_idx = self.import_count + self.local_func_count;
                                    // Register in functions map
                                    self.functions
                                        .insert(internal_name.clone(), (type_idx, func_idx, false));
                                    let export_name = format!(
                                        "{}/{}#[constructor]{}",
                                        package_path, iface.name.name, resource_name
                                    );
                                    self.exports.push((export_name, func_idx));
                                    self.func_bodies.push((iface.name.name.clone(), func));
                                    self.func_spans.push((func_idx, span.clone()));
                                    self.local_func_count += 1;
                                }
                                ResourceMethod::Method(func) => {
                                    // Instance method: [method]resource-name.method-name
                                    // Use unique internal name
                                    let internal_name =
                                        format!("[method]{}.{}", resource_name, func.name.name);
                                    let method_name = kettu_parser::Id {
                                        name: internal_name.clone(),
                                        span: func.name.span.clone(),
                                    };
                                    // Add implicit self: i32 as first param
                                    let mut params_with_self = vec![Param {
                                        name: kettu_parser::Id {
                                            name: "self".to_string(),
                                            span: func.name.span.clone(),
                                        },
                                        ty: Ty::Primitive(PrimitiveTy::S32, func.name.span.clone()),
                                    }];
                                    params_with_self.extend(func.params.clone());

                                    let method_func = Func {
                                        gates: func.gates.clone(),
                                        name: method_name,
                                        type_params: func.type_params.clone(),
                                        is_async: func.is_async,
                                        params: params_with_self,
                                        result: func.result.clone(),
                                        body: func.body.clone().or_else(|| {
                                            Some(FuncBody {
                                                statements: vec![],
                                                span: func.span.clone(),
                                            })
                                        }),
                                        span: func.span.clone(),
                                    };
                                    let type_idx = self.add_func_type(&method_func)?;
                                    let func_idx = self.import_count + self.local_func_count;
                                    // Register in functions map
                                    self.functions
                                        .insert(internal_name.clone(), (type_idx, func_idx, false));
                                    let export_name = format!(
                                        "{}/{}#[method]{}.{}",
                                        package_path,
                                        iface.name.name,
                                        resource_name,
                                        func.name.name
                                    );
                                    self.exports.push((export_name, func_idx));
                                    self.func_bodies
                                        .push((iface.name.name.clone(), method_func.clone()));
                                    self.func_spans.push((func_idx, method_func.span.clone()));
                                    self.local_func_count += 1;
                                }
                                ResourceMethod::Static(func) => {
                                    // Static method: [static]resource-name.method-name
                                    // Use unique internal name
                                    let internal_name =
                                        format!("[static]{}.{}", resource_name, func.name.name);
                                    let static_name = kettu_parser::Id {
                                        name: internal_name.clone(),
                                        span: func.name.span.clone(),
                                    };
                                    let static_func = Func {
                                        gates: func.gates.clone(),
                                        name: static_name,
                                        type_params: func.type_params.clone(),
                                        is_async: func.is_async,
                                        params: func.params.clone(),
                                        result: func.result.clone(),
                                        body: func.body.clone().or_else(|| {
                                            Some(FuncBody {
                                                statements: vec![],
                                                span: func.span.clone(),
                                            })
                                        }),
                                        span: func.span.clone(),
                                    };
                                    let type_idx = self.add_func_type(&static_func)?;
                                    let func_idx = self.import_count + self.local_func_count;
                                    // Register in functions map
                                    self.functions
                                        .insert(internal_name.clone(), (type_idx, func_idx, false));
                                    let export_name = format!(
                                        "{}/{}#[static]{}.{}",
                                        package_path,
                                        iface.name.name,
                                        resource_name,
                                        func.name.name
                                    );
                                    self.exports.push((export_name, func_idx));
                                    self.func_bodies
                                        .push((iface.name.name.clone(), static_func.clone()));
                                    self.func_spans.push((func_idx, static_func.span.clone()));
                                    self.local_func_count += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn add_func_type(&mut self, func: &Func) -> Result<u32, CompileError> {
        // Flatten params using canonical ABI: string/list expand to (ptr, len)
        let mut params: Vec<ValType> = Vec::new();
        for p in &func.params {
            params.extend(self.ty_to_valtypes(&p.ty)?);
        }

        // For async functions using stackless ABI (when --wasip3 is enabled):
        // - Return i32 (status code: 0=done, 1=yield, 2=wait)
        // - Results are passed via task.return built-in
        // Without --wasip3: compile async functions with sync ABI
        let results: Vec<ValType> = if func.is_async && self.options.wasip3 {
            vec![ValType::I32] // status code
        } else if let Some(ref ty) = func.result {
            vec![self.ty_to_valtype(ty)?]
        } else {
            vec![]
        };

        // Check if type already exists
        for (i, (p, r)) in self.types.iter().enumerate() {
            if p == &params && r == &results {
                return Ok(i as u32);
            }
        }

        // Add new type
        let idx = self.types.len() as u32;
        self.types.push((params, results));
        Ok(idx)
    }

    fn ty_to_valtype(&self, ty: &Ty) -> Result<ValType, CompileError> {
        match ty {
            Ty::Primitive(prim, _) => match prim {
                PrimitiveTy::U8
                | PrimitiveTy::U16
                | PrimitiveTy::U32
                | PrimitiveTy::S8
                | PrimitiveTy::S16
                | PrimitiveTy::S32
                | PrimitiveTy::Bool
                | PrimitiveTy::Char => Ok(ValType::I32),
                PrimitiveTy::U64 | PrimitiveTy::S64 => Ok(ValType::I64),
                PrimitiveTy::F32 => Ok(ValType::F32),
                PrimitiveTy::F64 => Ok(ValType::F64),
                PrimitiveTy::String => Ok(ValType::I32), // String is a pointer
                PrimitiveTy::V128 => Ok(ValType::V128),
            },
            Ty::Named(_) => Ok(ValType::I32), // Named types are pointers
            Ty::List { .. }
            | Ty::Option { .. }
            | Ty::Result { .. }
            | Ty::Tuple { .. }
            | Ty::Future { .. }
            | Ty::Stream { .. }
            | Ty::Borrow { .. }
            | Ty::Own { .. }
            | Ty::Generic { .. } => Ok(ValType::I32), // All reference types are i32 pointers
        }
    }

    /// Flatten a WIT type into its canonical ABI core WASM types.
    /// Unlike ty_to_valtype (single value), this returns the full
    /// multi-value lowering: string/list → [I32, I32] (ptr + len).
    fn ty_to_valtypes(&self, ty: &Ty) -> Result<Vec<ValType>, CompileError> {
        match ty {
            Ty::Primitive(PrimitiveTy::String, _) => Ok(vec![ValType::I32, ValType::I32]),
            Ty::List { .. } => Ok(vec![ValType::I32, ValType::I32]),
            _ => Ok(vec![self.ty_to_valtype(ty)?]),
        }
    }

    /// Check if an expression is of string type (for codegen decisions)
    fn is_string_expr(expr: &Expr) -> bool {
        match expr {
            Expr::String(_, _) => true,
            Expr::InterpolatedString(_, _) => true,
            // Binary Add of strings produces a string
            Expr::Binary {
                lhs,
                op: kettu_parser::BinOp::Add,
                ..
            } => Self::is_string_expr(lhs),
            // For now, we can't determine type of identifiers without type info
            // The type checker has validated, so we'll rely on that
            _ => false,
        }
    }

    /// Register a string literal and return its (offset, length)
    /// Strings are stored as: [4-byte length LE][data bytes]
    /// The returned offset points to the data, length is at offset-4
    fn register_string(&mut self, s: &str) -> (u32, u32) {
        let str_bytes = s.as_bytes();
        let len = str_bytes.len() as u32;

        // Build: [len_le32][string_data]
        let mut bytes = Vec::with_capacity(4 + str_bytes.len());
        bytes.extend_from_slice(&len.to_le_bytes());
        bytes.extend_from_slice(str_bytes);

        let data_offset = self.string_offset + 4; // point to data, not length
        self.string_data.push((self.string_offset, bytes));
        self.string_offset += 4 + len;
        (data_offset, len)
    }

    /// Ensure the alloc function exists and return its index
    fn ensure_alloc_func(&mut self) -> u32 {
        if let Some(idx) = self.alloc_func_idx {
            return idx;
        }

        // Add alloc type: cabi_realloc(ptr: i32, old_size: i32, align: i32, new_size: i32) -> i32
        let type_idx = self.types.len() as u32;
        self.types.push((
            vec![ValType::I32, ValType::I32, ValType::I32, ValType::I32],
            vec![ValType::I32],
        ));

        // Add function at the end of user functions
        let func_idx = self.import_count + self.local_func_count;
        self.local_func_count += 1;
        self.alloc_func_idx = Some(func_idx);
        self.functions
            .insert("$alloc".to_string(), (type_idx, func_idx, false));
        self.exports.push(("cabi_realloc".to_string(), func_idx));

        func_idx
    }

    /// Build the alloc function body (bump allocator)
    /// cabi_realloc(ptr: i32, old_size: i32, align: i32, new_size: i32) -> i32
    fn build_alloc_function(&self) -> Function {
        // For a simple bump allocator, we ignore ptr, old_size, align and just allocate new_size bytes
        // ptr = global.get $heap_ptr
        // global.set $heap_ptr (ptr + new_size)
        // return ptr
        let mut function = Function::new(vec![(1, ValType::I32)]); // 1 local for ptr

        // ptr = global.get $heap_ptr (global 0)
        function.instruction(&Instruction::GlobalGet(0));
        function.instruction(&Instruction::LocalSet(4)); // store in local 4 (after 4 params)

        // global.set $heap_ptr (ptr + new_size)
        function.instruction(&Instruction::LocalGet(4)); // ptr
        function.instruction(&Instruction::LocalGet(3)); // new_size (param 3)
        function.instruction(&Instruction::I32Add);
        function.instruction(&Instruction::GlobalSet(0));

        // return ptr
        function.instruction(&Instruction::LocalGet(4));
        function.instruction(&Instruction::End);

        function
    }

    /// Ensure the str_concat function exists and return its index
    fn ensure_str_concat_func(&mut self) -> u32 {
        if let Some(idx) = self.str_concat_func_idx {
            return idx;
        }

        // Make sure alloc is available
        self.ensure_alloc_func();

        // Add str_concat type: (ptr1: i32, ptr2: i32) -> i32
        let type_idx = self.types.len() as u32;
        self.types
            .push((vec![ValType::I32, ValType::I32], vec![ValType::I32]));

        // Add function
        let func_idx = self.import_count + self.local_func_count;
        self.local_func_count += 1;
        self.str_concat_func_idx = Some(func_idx);
        self.functions
            .insert("$str_concat".to_string(), (type_idx, func_idx, false));

        func_idx
    }

    /// Build the str_concat function body
    /// String format: [4-byte length LE][data bytes], pointer points to data
    /// str_concat(ptr1: i32, ptr2: i32) -> i32
    fn build_str_concat_function(&self) -> Function {
        // Locals: 0=ptr1, 1=ptr2, 2=len1, 3=len2, 4=result, 5=total_len
        let mut function = Function::new(vec![(4, ValType::I32)]); // 4 locals

        // len1 = i32.load(ptr1 - 4)
        function.instruction(&Instruction::LocalGet(0));
        function.instruction(&Instruction::I32Const(4));
        function.instruction(&Instruction::I32Sub);
        function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
            offset: 0,
            align: 2, // 4-byte aligned
            memory_index: 0,
        }));
        function.instruction(&Instruction::LocalSet(2)); // len1

        // len2 = i32.load(ptr2 - 4)
        function.instruction(&Instruction::LocalGet(1));
        function.instruction(&Instruction::I32Const(4));
        function.instruction(&Instruction::I32Sub);
        function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
        function.instruction(&Instruction::LocalSet(3)); // len2

        // total_len = len1 + len2
        function.instruction(&Instruction::LocalGet(2));
        function.instruction(&Instruction::LocalGet(3));
        function.instruction(&Instruction::I32Add);
        function.instruction(&Instruction::LocalSet(5)); // total_len

        // result = alloc(total_len + 4)
        // Push CABI args: (0, 0, 0, size)
        function.instruction(&Instruction::I32Const(0)); // ptr (unused)
        function.instruction(&Instruction::I32Const(0)); // old_size (unused)
        function.instruction(&Instruction::I32Const(0)); // align (unused)
        function.instruction(&Instruction::LocalGet(5));
        function.instruction(&Instruction::I32Const(4));
        function.instruction(&Instruction::I32Add);
        function.instruction(&Instruction::Call(self.alloc_func_idx.unwrap()));
        function.instruction(&Instruction::LocalSet(4)); // result

        // Store total_len at result
        function.instruction(&Instruction::LocalGet(4));
        function.instruction(&Instruction::LocalGet(5));
        function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));

        // memory.copy(result+4, ptr1, len1)
        function.instruction(&Instruction::LocalGet(4));
        function.instruction(&Instruction::I32Const(4));
        function.instruction(&Instruction::I32Add);
        function.instruction(&Instruction::LocalGet(0)); // ptr1 (source)
        function.instruction(&Instruction::LocalGet(2)); // len1
        function.instruction(&Instruction::MemoryCopy {
            src_mem: 0,
            dst_mem: 0,
        });

        // memory.copy(result+4+len1, ptr2, len2)
        function.instruction(&Instruction::LocalGet(4));
        function.instruction(&Instruction::I32Const(4));
        function.instruction(&Instruction::I32Add);
        function.instruction(&Instruction::LocalGet(2)); // len1
        function.instruction(&Instruction::I32Add);
        function.instruction(&Instruction::LocalGet(1)); // ptr2 (source)
        function.instruction(&Instruction::LocalGet(3)); // len2
        function.instruction(&Instruction::MemoryCopy {
            src_mem: 0,
            dst_mem: 0,
        });

        // return result + 4 (pointer to data)
        function.instruction(&Instruction::LocalGet(4));
        function.instruction(&Instruction::I32Const(4));
        function.instruction(&Instruction::I32Add);
        function.instruction(&Instruction::End);

        function
    }

    /// Ensure the str_eq function exists and return its index
    fn ensure_str_eq_func(&mut self) -> u32 {
        if let Some(idx) = self.str_eq_func_idx {
            return idx;
        }

        // Add str_eq type: (ptr1: i32, ptr2: i32) -> i32
        let type_idx = self.types.len() as u32;
        self.types
            .push((vec![ValType::I32, ValType::I32], vec![ValType::I32]));

        // Add function
        let func_idx = self.import_count + self.local_func_count;
        self.local_func_count += 1;
        self.str_eq_func_idx = Some(func_idx);
        self.functions
            .insert("$str_eq".to_string(), (type_idx, func_idx, false));

        func_idx
    }

    /// Build the str_eq function body
    /// String format: [4-byte length LE][data bytes], pointer points to data (length at ptr-4)
    /// str_eq(ptr1: i32, ptr2: i32) -> i32 (1=equal, 0=not equal)
    fn build_str_eq_function(&self) -> Function {
        // Locals: 0=ptr1, 1=ptr2, 2=len1, 3=idx
        let mut function = Function::new(vec![(2, ValType::I32)]); // 2 extra locals: len1, idx

        // Fast path: if ptr1 == ptr2, return 1
        function.instruction(&Instruction::LocalGet(0));
        function.instruction(&Instruction::LocalGet(1));
        function.instruction(&Instruction::I32Eq);
        function.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
        function.instruction(&Instruction::I32Const(1));
        function.instruction(&Instruction::Return);
        function.instruction(&Instruction::End);

        // Load len1 = i32.load(ptr1 - 4)
        function.instruction(&Instruction::LocalGet(0));
        function.instruction(&Instruction::I32Const(4));
        function.instruction(&Instruction::I32Sub);
        function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
            offset: 0, align: 2, memory_index: 0,
        }));
        function.instruction(&Instruction::LocalSet(2));

        // Load len2 = i32.load(ptr2 - 4)
        function.instruction(&Instruction::LocalGet(1));
        function.instruction(&Instruction::I32Const(4));
        function.instruction(&Instruction::I32Sub);
        function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
            offset: 0, align: 2, memory_index: 0,
        }));

        // If len1 != len2, return 0
        function.instruction(&Instruction::LocalGet(2));
        function.instruction(&Instruction::I32Ne);
        function.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
        function.instruction(&Instruction::I32Const(0));
        function.instruction(&Instruction::Return);
        function.instruction(&Instruction::End);

        // idx = 0
        function.instruction(&Instruction::I32Const(0));
        function.instruction(&Instruction::LocalSet(3));

        // Byte comparison loop
        function.instruction(&Instruction::Block(wasm_encoder::BlockType::Empty));
        function.instruction(&Instruction::Loop(wasm_encoder::BlockType::Empty));

        // if idx >= len1, break (equal)
        function.instruction(&Instruction::LocalGet(3));
        function.instruction(&Instruction::LocalGet(2));
        function.instruction(&Instruction::I32GeU);
        function.instruction(&Instruction::BrIf(1));

        // Compare bytes: ptr1[idx] vs ptr2[idx]
        function.instruction(&Instruction::LocalGet(0));
        function.instruction(&Instruction::LocalGet(3));
        function.instruction(&Instruction::I32Add);
        function.instruction(&Instruction::I32Load8U(wasm_encoder::MemArg {
            offset: 0, align: 0, memory_index: 0,
        }));
        function.instruction(&Instruction::LocalGet(1));
        function.instruction(&Instruction::LocalGet(3));
        function.instruction(&Instruction::I32Add);
        function.instruction(&Instruction::I32Load8U(wasm_encoder::MemArg {
            offset: 0, align: 0, memory_index: 0,
        }));
        function.instruction(&Instruction::I32Ne);
        function.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
        function.instruction(&Instruction::I32Const(0));
        function.instruction(&Instruction::Return);
        function.instruction(&Instruction::End);

        // idx++
        function.instruction(&Instruction::LocalGet(3));
        function.instruction(&Instruction::I32Const(1));
        function.instruction(&Instruction::I32Add);
        function.instruction(&Instruction::LocalSet(3));

        function.instruction(&Instruction::Br(0));
        function.instruction(&Instruction::End); // end loop
        function.instruction(&Instruction::End); // end block

        // All bytes matched
        function.instruction(&Instruction::I32Const(1));
        function.instruction(&Instruction::End);

        function
    }

    /// Ensure the arena_reset function exists and return its index
    fn ensure_arena_reset_func(&mut self) -> u32 {
        if let Some(idx) = self.arena_reset_func_idx {
            return idx;
        }

        // Add arena_reset type: () -> ()
        let type_idx = self.types.len() as u32;
        self.types.push((vec![], vec![]));

        // Add function
        let func_idx = self.import_count + self.local_func_count;
        self.local_func_count += 1;
        self.arena_reset_func_idx = Some(func_idx);
        self.functions
            .insert("$arena_reset".to_string(), (type_idx, func_idx, false));
        self.exports
            .push(("cabi_arena_reset".to_string(), func_idx));

        func_idx
    }

    /// Build the arena_reset function body
    /// Resets heap pointer to initial value (after static string data)
    fn build_arena_reset_function(&self, initial_offset: u32) -> Function {
        let mut function = Function::new(vec![]);

        // global.set $heap_ptr <initial_offset>
        function.instruction(&Instruction::I32Const(initial_offset as i32));
        function.instruction(&Instruction::GlobalSet(0));
        function.instruction(&Instruction::End);

        function
    }

    /// Ensure the task.return built-in import exists for the given result type
    /// Returns the function index for task.return with those result types
    ///
    /// task.return signature: (result_values...) -> ()
    /// For async exports, we call this instead of using normal return
    /// NOTE: Reserved for full async ABI implementation (WASI Preview 3)
    #[allow(dead_code)]
    fn ensure_task_return_import(&mut self, result_types: &[ValType]) -> u32 {
        // Check if we already have a task.return for this signature
        if let Some(&func_idx) = self.task_return_types.get(result_types) {
            return func_idx;
        }

        // Create type: (result_types...) -> ()
        let type_idx = self.get_or_create_type(result_types, &[]);

        // Register as an import from "canon-async" interface
        // Note: This will be lowered by wit-component's canon task.return
        let func_idx = self.import_count;
        self.import_count += 1;

        // Track this for later import section generation
        let func_name = format!("$task_return_{}", self.task_return_types.len());
        self.functions
            .insert(func_name.clone(), (type_idx, func_idx, true));
        self.task_return_func_idx = Some(func_idx);
        self.task_return_types
            .insert(result_types.to_vec(), func_idx);

        func_idx
    }

    /// Find or create a type index for the given parameter and result types
    fn get_or_create_type(&mut self, params: &[ValType], results: &[ValType]) -> u32 {
        self.types
            .iter()
            .position(|(p, r)| p.as_slice() == params && r.as_slice() == results)
            .map(|i| i as u32)
            .unwrap_or_else(|| {
                let idx = self.types.len() as u32;
                self.types.push((params.to_vec(), results.to_vec()));
                idx
            })
    }

    /// Ensure waitable-set.new import exists, return its function index
    /// waitable-set.new: () -> i32 (returns waitable-set index)
    fn ensure_waitable_set_new_import(&mut self) -> u32 {
        if let Some(idx) = self.waitable_set_new_idx {
            return idx;
        }

        // () -> i32
        let type_idx = self.get_or_create_type(&[], &[ValType::I32]);
        let func_idx = self.import_count;
        self.import_count += 1;

        self.functions
            .insert("$waitable_set_new".to_string(), (type_idx, func_idx, true));
        self.waitable_set_new_idx = Some(func_idx);

        func_idx
    }

    /// Ensure waitable-set.wait import exists, return its function index
    /// waitable-set.wait: (waitable_set: i32, out_ptr: i32) -> i32 (blocks until event)
    fn ensure_waitable_set_wait_import(&mut self) -> u32 {
        if let Some(idx) = self.waitable_set_wait_idx {
            return idx;
        }

        // (i32, i32) -> i32
        let type_idx = self.get_or_create_type(&[ValType::I32, ValType::I32], &[ValType::I32]);
        let func_idx = self.import_count;
        self.import_count += 1;

        self.functions
            .insert("$waitable_set_wait".to_string(), (type_idx, func_idx, true));
        self.waitable_set_wait_idx = Some(func_idx);

        func_idx
    }

    /// Ensure subtask.drop import exists, return its function index
    /// subtask.drop: (subtask: i32) -> ()
    fn ensure_subtask_drop_import(&mut self) -> u32 {
        if let Some(idx) = self.subtask_drop_idx {
            return idx;
        }

        // (i32) -> ()
        let type_idx = self.get_or_create_type(&[ValType::I32], &[]);
        let func_idx = self.import_count;
        self.import_count += 1;

        self.functions
            .insert("$subtask_drop".to_string(), (type_idx, func_idx, true));
        self.subtask_drop_idx = Some(func_idx);

        func_idx
    }


    /// Ensure thread-spawn import exists, return its function index
    fn ensure_thread_spawn_import(&mut self) -> u32 {
        if let Some(idx) = self.thread_spawn_idx {
            return idx;
        }
        let type_idx = self.get_or_create_type(&[ValType::I32], &[ValType::I32]);
        let func_idx = self.import_count;
        self.import_count += 1;
        self.functions
            .insert("$thread_spawn".to_string(), (type_idx, func_idx, true));
        self.thread_spawn_idx = Some(func_idx);
        func_idx
    }

    /// Count the number of await points in a function body
    fn count_await_points_in_func(func: &Func) -> usize {
        fn count_in_expr(expr: &Expr) -> usize {
            match expr {
                Expr::Await { expr, .. } => 1 + count_in_expr(expr),
                Expr::Binary { lhs, rhs, .. } => count_in_expr(lhs) + count_in_expr(rhs),
                Expr::Not(inner, _) | Expr::Neg(inner, _) => count_in_expr(inner),
                Expr::If {
                    cond,
                    then_branch,
                    else_branch,
                    ..
                } => {
                    count_in_expr(cond)
                        + then_branch.iter().map(count_in_stmt).sum::<usize>()
                        + else_branch
                            .as_ref()
                            .map_or(0, |stmts| stmts.iter().map(count_in_stmt).sum())
                }
                Expr::Call { func, args, .. } => {
                    count_in_expr(func) + args.iter().map(count_in_expr).sum::<usize>()
                }
                Expr::Match {
                    scrutinee, arms, ..
                } => {
                    count_in_expr(scrutinee)
                        + arms
                            .iter()
                            .map(|arm| arm.body.iter().map(count_in_stmt).sum::<usize>())
                            .sum::<usize>()
                }
                Expr::While {
                    condition, body, ..
                } => count_in_expr(condition) + body.iter().map(count_in_stmt).sum::<usize>(),
                Expr::For { range, body, .. } => {
                    count_in_expr(range) + body.iter().map(count_in_stmt).sum::<usize>()
                }
                Expr::ForEach {
                    collection, body, ..
                } => count_in_expr(collection) + body.iter().map(count_in_stmt).sum::<usize>(),
                Expr::Range {
                    start, end, step, ..
                } => {
                    count_in_expr(start)
                        + count_in_expr(end)
                        + step.as_ref().map_or(0, |e| count_in_expr(e))
                }
                Expr::ListLiteral { elements, .. } => elements.iter().map(count_in_expr).sum(),
                Expr::Index { expr, index, .. } => count_in_expr(expr) + count_in_expr(index),
                Expr::Slice {
                    expr, start, end, ..
                } => count_in_expr(expr) + count_in_expr(start) + count_in_expr(end),
                Expr::Lambda { body, .. } => count_in_expr(body),
                Expr::RecordLiteral { fields, .. } => {
                    fields.iter().map(|(_, e)| count_in_expr(e)).sum()
                }
                Expr::Field { expr, .. } => count_in_expr(expr),
                Expr::OptionalChain { expr, .. } => count_in_expr(expr),
                Expr::Try { expr, .. } => count_in_expr(expr),
                Expr::Map { list, lambda, .. } => count_in_expr(list) + count_in_expr(lambda),
                Expr::Filter { list, lambda, .. } => count_in_expr(list) + count_in_expr(lambda),
                Expr::Reduce {
                    list, init, lambda, ..
                } => count_in_expr(list) + count_in_expr(init) + count_in_expr(lambda),
                Expr::Assert(inner, _) | Expr::StrLen(inner, _) | Expr::ListLen(inner, _) => {
                    count_in_expr(inner)
                }
                Expr::StrEq(a, b, _) | Expr::ListPush(a, b, _) => {
                    count_in_expr(a) + count_in_expr(b)
                }
                Expr::ListSet(list, idx, val, _) => {
                    count_in_expr(list) + count_in_expr(idx) + count_in_expr(val)
                }
                Expr::VariantLiteral { payload, .. } => {
                    payload.as_ref().map_or(0, |e| count_in_expr(e))
                }
                Expr::InterpolatedString(parts, _) => parts
                    .iter()
                    .map(|p| match p {
                        StringPart::Expr(e) => count_in_expr(e),
                        _ => 0,
                    })
                    .sum(),
                // Leaf expressions - no sub-expressions
                Expr::AtomicLoad { .. } | Expr::AtomicStore { .. } | Expr::AtomicAdd { .. }
                | Expr::AtomicSub { .. } | Expr::AtomicCmpxchg { .. }
                | Expr::AtomicWait { .. } | Expr::AtomicNotify { .. }
                | Expr::ThreadJoin { .. } => 0,
                Expr::Spawn { body, .. } | Expr::AtomicBlock { body, .. } => {
                    body.iter().map(count_in_stmt).sum()
                }
                Expr::SimdOp { args, .. } => args.iter().map(count_in_expr).sum(),
                Expr::SimdForEach { collection, body, .. } => {
                    count_in_expr(collection) + body.iter().map(count_in_stmt).sum::<usize>()
                }
                Expr::Ident(_) | Expr::Integer(_, _) | Expr::String(_, _) | Expr::Bool(_, _) => 0,
            }
        }

        fn count_in_stmt(stmt: &Statement) -> usize {
            match stmt {
                Statement::Expr(e) => count_in_expr(e),
                Statement::Let { value, .. } => count_in_expr(value),
                Statement::Assign { value, .. } | Statement::CompoundAssign { value, .. } => count_in_expr(value),
                Statement::Return(Some(e)) => count_in_expr(e),
                Statement::Return(None) | Statement::Break { .. } | Statement::Continue { .. } => 0,
                Statement::SharedLet { initial_value, .. } => count_in_expr(initial_value),
            }
        }

        if let Some(ref body) = func.body {
            body.statements.iter().map(count_in_stmt).sum()
        } else {
            0
        }
    }

    /// Check if a function contains any await points
    fn has_await_points(func: &Func) -> bool {
        Self::count_await_points_in_func(func) > 0
    }

    /// Allocate space in linear memory for async state saving
    /// Returns the base offset for this function's state
    fn alloc_async_state(&mut self, num_locals: u32) -> u32 {
        let offset = self.async_state_offset;
        // State layout: [state_id: i32] [locals: i32 * num_locals]
        self.async_state_offset += 4 + (num_locals * 4);
        offset
    }

    /// Emit instructions to save locals to memory at given offset
    #[allow(dead_code)]
    fn emit_save_locals(
        function: &mut wasm_encoder::Function,
        locals: &HashMap<String, u32>,
        state_offset: u32,
    ) {
        // Save each local to memory
        for (_name, &local_idx) in locals.iter() {
            // Compute address: state_offset + 4 + (local_idx * 4)
            function.instruction(&Instruction::I32Const(
                (state_offset + 4 + local_idx * 4) as i32,
            ));
            function.instruction(&Instruction::LocalGet(local_idx));
            function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
        }
    }

    /// Emit instructions to restore locals from memory at given offset
    #[allow(dead_code)]
    fn emit_restore_locals(
        function: &mut wasm_encoder::Function,
        locals: &HashMap<String, u32>,
        state_offset: u32,
    ) {
        // Restore each local from memory
        for (_name, &local_idx) in locals.iter() {
            // Compute address: state_offset + 4 + (local_idx * 4)
            function.instruction(&Instruction::I32Const(
                (state_offset + 4 + local_idx * 4) as i32,
            ));
            function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            function.instruction(&Instruction::LocalSet(local_idx));
        }
    }

    /// Emit a callback function for an async export
    /// Callback signature: (event: i32, p1: i32, p2: i32) -> i32
    /// Returns the table index of the callback
    fn emit_async_callback(
        &mut self,
        func_name: &str,
        func: &Func,
        _state_offset: u32,
        num_locals: u32,
    ) -> Result<u32, CompileError> {
        // Callback type: (i32, i32, i32) -> i32
        let type_idx =
            self.get_or_create_type(&[ValType::I32, ValType::I32, ValType::I32], &[ValType::I32]);

        // Allocate function index
        let func_idx = self.import_count + self.local_func_count;
        self.local_func_count += 1;

        // Register callback
        let callback_name = format!("{}$callback", func_name);
        self.functions
            .insert(callback_name.clone(), (type_idx, func_idx, false));

        // Store for later body compilation
        self.callback_bodies
            .push((func_name.to_string(), func.clone(), num_locals));

        // Export the callback
        self.exports.push((callback_name, func_idx));

        // Return table index (for indirect call)
        Ok(func_idx + 1) // +1 because table index 0 is null
    }

    /// Emit a lambda as a separate WASM function and return its table index (1-based, 0 is null)
    fn emit_lambda(
        &mut self,
        captures: &[kettu_parser::Id],
        params: &[kettu_parser::Id],
        body: &Expr,
    ) -> Result<u32, CompileError> {
        // Build param types: captures first, then regular params (all i32 for now)
        let total_params = captures.len() + params.len();
        let param_types: Vec<ValType> = (0..total_params).map(|_| ValType::I32).collect();
        let result_types = vec![ValType::I32]; // All lambdas return i32 for now

        // Check if we already have this type
        let type_idx = self
            .types
            .iter()
            .position(|(p, r)| p == &param_types && r == &result_types)
            .map(|i| i as u32)
            .unwrap_or_else(|| {
                let idx = self.types.len() as u32;
                self.types.push((param_types.clone(), result_types.clone()));
                idx
            });

        // Lambda function index: imports + all local funcs (user + builtins) + current lambda count
        // local_func_count includes user funcs AND builtin funcs added in Phase 2
        let lambda_index = self.lambda_bodies.len() as u32;
        let func_idx = self.import_count + self.local_func_count + lambda_index;

        // Table index is 1-based (0 is null)
        let table_idx = lambda_index + 1;

        // Store the lambda body for later compilation (captures, params, body)
        self.lambda_bodies.push((
            type_idx,
            func_idx,
            captures.to_vec(),
            params.to_vec(),
            body.clone(),
        ));

        self.next_lambda_id += 1;

        Ok(table_idx)
    }

    fn compile_function(&mut self, func: &Func) -> Result<Function, CompileError> {
        // Check if this async function has await points - if so, we need a callback
        let needs_callback = func.is_async && self.options.wasip3 && Self::has_await_points(func);

        // Build local variable map: param_name -> index, let_name -> index
        // For canonical ABI, string/list params expand to 2 WASM locals (ptr, len).
        // We map each WIT param name to either:
        //   - The WASM local directly (for scalar types)
        //   - A synthetic local that will hold the internal-format ptr (for string/list)
        let mut locals: HashMap<String, u32> = HashMap::new();
        let mut let_count = 0u32;

        // Track which params need canonical → internal conversion
        // (wit_param_name, ptr_local, len_local, synthetic_local will be assigned later)
        let mut string_param_conversions: Vec<(String, u32, u32)> = Vec::new();

        // Parameters get indices 0..n, but string/list params take 2 WASM locals
        let mut wasm_local_idx = 0u32;
        for param in func.params.iter() {
            let is_wide = matches!(&param.ty,
                Ty::Primitive(PrimitiveTy::String, _) | Ty::List { .. }
            );
            if is_wide {
                // String/list: 2 WASM locals (ptr, len). Name maps to a synthetic local later.
                let ptr_local = wasm_local_idx;
                let len_local = wasm_local_idx + 1;
                string_param_conversions.push((param.name.name.clone(), ptr_local, len_local));
                wasm_local_idx += 2;
                // Don't insert into locals yet - will point to synthetic local
            } else {
                locals.insert(param.name.name.clone(), wasm_local_idx);
                wasm_local_idx += 1;
            }
        }
        let wasm_params_len = wasm_local_idx as usize;

        let mut v128_locals = std::collections::HashSet::new();

        // Count let bindings and for loop variables in the body
        if let Some(ref body) = func.body {
            fn collect_locals_from_expr(
                expr: &Expr,
                params_len: usize,
                locals: &mut HashMap<String, u32>,
                let_count: &mut u32,
                v128_locals: &mut std::collections::HashSet<u32>,
            ) {
                match expr {
                    Expr::For { variable, body, .. } => {
                        // For loop variable
                        locals.insert(variable.name.clone(), params_len as u32 + *let_count);
                        *let_count += 1;
                        // Scan body statements
                        for stmt in body {
                            collect_locals_from_stmt(stmt, params_len, locals, let_count, v128_locals);
                        }
                    }
                    Expr::ForEach { variable, body, .. } => {
                        // For-each loop variable
                        locals.insert(variable.name.clone(), params_len as u32 + *let_count);
                        *let_count += 1;
                        // Also need temp locals for list_ptr and idx (2 more)
                        *let_count += 2;
                        // Scan body statements
                        for stmt in body {
                            collect_locals_from_stmt(stmt, params_len, locals, let_count, v128_locals);
                        }
                    }
                    Expr::SimdForEach { variable, body, .. } => {
                        // SIMD for-each loop variable (v128 type)
                        let idx = params_len as u32 + *let_count;
                        locals.insert(variable.name.clone(), idx);
                        v128_locals.insert(idx);
                        *let_count += 1;
                        // Need temp locals: list_ptr, idx, end (3 more)
                        *let_count += 3;
                        // Scan body statements
                        for stmt in body {
                            collect_locals_from_stmt(stmt, params_len, locals, let_count, v128_locals);
                        }
                    }
                    Expr::While { body, .. } => {
                        for stmt in body {
                            collect_locals_from_stmt(stmt, params_len, locals, let_count, v128_locals);
                        }
                    }
                    Expr::If {
                        then_branch,
                        else_branch,
                        ..
                    } => {
                        for stmt in then_branch {
                            collect_locals_from_stmt(stmt, params_len, locals, let_count, v128_locals);
                        }
                        if let Some(else_stmts) = else_branch {
                            for stmt in else_stmts {
                                collect_locals_from_stmt(stmt, params_len, locals, let_count, v128_locals);
                            }
                        }
                    }
                    Expr::Slice { .. } => {
                        // Slice needs 6 temp locals: src_ptr, start, end, len, dest_ptr, i
                        *let_count += 6;
                    }
                    Expr::ListPush(_, _, _) => {
                        // ListPush needs 5 temp locals: src_ptr, len, dest_ptr, i, val
                        *let_count += 5;
                    }
                    Expr::Map { list, lambda, .. } => {
                        // Map needs 5 temp locals: src_ptr, len, dest_ptr, i, elem
                        *let_count += 5;
                        // Also collect from the list expression
                        collect_locals_from_expr(list, params_len, locals, let_count, v128_locals);
                        // And from the lambda body
                        if let Expr::Lambda { body, .. } = lambda.as_ref() {
                            collect_locals_from_expr(body, params_len, locals, let_count, v128_locals);
                        }
                    }
                    Expr::Filter { list, lambda, .. } => {
                        // Filter needs 6 temp locals: src_ptr, len, dest_ptr, i, j, elem
                        *let_count += 6;
                        collect_locals_from_expr(list, params_len, locals, let_count, v128_locals);
                        if let Expr::Lambda { body, .. } = lambda.as_ref() {
                            collect_locals_from_expr(body, params_len, locals, let_count, v128_locals);
                        }
                    }
                    Expr::Reduce {
                        list, init, lambda, ..
                    } => {
                        // Reduce needs 5 temp locals: src_ptr, len, i, acc, elem
                        *let_count += 5;
                        collect_locals_from_expr(list, params_len, locals, let_count, v128_locals);
                        collect_locals_from_expr(init, params_len, locals, let_count, v128_locals);
                        if let Expr::Lambda { body, .. } = lambda.as_ref() {
                            collect_locals_from_expr(body, params_len, locals, let_count, v128_locals);
                        }
                    }
                    _ => {}
                }
            }

            fn collect_locals_from_stmt(
                stmt: &Statement,
                params_len: usize,
                locals: &mut HashMap<String, u32>,
                let_count: &mut u32,
                v128_locals: &mut std::collections::HashSet<u32>,
            ) {
                match stmt {
                    Statement::Let { name, value } => {
                        let idx = params_len as u32 + *let_count;
                        locals.insert(name.name.clone(), idx);
                        if expr_is_v128(value) {
                            v128_locals.insert(idx);
                        }
                        *let_count += 1;
                        collect_locals_from_expr(value, params_len, locals, let_count, v128_locals);
                    }
                    Statement::Expr(e) => {
                        collect_locals_from_expr(e, params_len, locals, let_count, v128_locals);
                    }
                    Statement::Assign { value, .. } | Statement::CompoundAssign { value, .. } => {
                        collect_locals_from_expr(value, params_len, locals, let_count, v128_locals);
                    }
                    Statement::SharedLet { name, initial_value } => {
                        locals.insert(name.name.clone(), params_len as u32 + *let_count);
                        *let_count += 1;
                        collect_locals_from_expr(initial_value, params_len, locals, let_count, v128_locals);
                    }
                    _ => {}
                }
            }

            /// Check if an expression produces a v128 SIMD value
            fn expr_is_v128(expr: &Expr) -> bool {
                match expr {
                    Expr::SimdOp { op, .. } => {
                        // Most SIMD ops return v128; extract_lane/tests return i32
                        !matches!(op, SimdOp::ExtractLane | SimdOp::AnyTrue | SimdOp::AllTrue | SimdOp::Bitmask)
                    }
                    Expr::Ident(_id) => {
                        // If it's referencing a known v128 variable, it's v128
                        // We can't easily tell here, so be conservative
                        false
                    }
                    _ => false,
                }
            }


            for stmt in &body.statements {
                collect_locals_from_stmt(stmt, wasm_params_len, &mut locals, &mut let_count, &mut v128_locals);
            }
        }

        // Add synthetic locals for string/list param conversions
        let num_string_conversions = string_param_conversions.len() as u32;
        let string_synth_base = wasm_params_len as u32 + let_count;
        // Assign synthetic local indices and insert into locals map
        for (i, (name, _ptr_local, _len_local)) in string_param_conversions.iter().enumerate() {
            let synth_idx = string_synth_base + i as u32;
            locals.insert(name.clone(), synth_idx);
        }

        // Declare locals with correct types (v128 for SIMD, i32 for everything else)
        // +1 for temp record pointer
        // +3 for match expressions (scrutinee + binding + spare)
        let extra_locals = 4;
        let total_declared = let_count + extra_locals + num_string_conversions;
        let local_types: Vec<_> = (0..total_declared)
            .map(|i| {
                let idx = wasm_params_len as u32 + i;
                if v128_locals.contains(&idx) {
                    (1, ValType::V128)
                } else {
                    (1, ValType::I32)
                }
            })
            .collect();
        let mut function = Function::new(local_types);

        // Emit preamble: convert canonical ABI string/list params to internal format.
        // Canonical: (ptr, len) where ptr points to raw UTF-8 data
        // Internal: [4-byte len LE][data], pointer points to data (len at ptr-4)
        for (name, ptr_local, len_local) in &string_param_conversions {
            let synth_idx = *locals.get(name).unwrap();
            let alloc_idx = self.ensure_alloc_func();
            // new_base = cabi_realloc(0, 0, 1, len + 4)
            function.instruction(&Instruction::I32Const(0));
            function.instruction(&Instruction::I32Const(0));
            function.instruction(&Instruction::I32Const(1));
            function.instruction(&Instruction::LocalGet(*len_local));
            function.instruction(&Instruction::I32Const(4));
            function.instruction(&Instruction::I32Add);
            function.instruction(&Instruction::Call(alloc_idx));
            // Stack: new_base
            // Store new_base in synthetic local temporarily
            function.instruction(&Instruction::LocalSet(synth_idx));
            // i32.store(new_base, len) — write length prefix
            function.instruction(&Instruction::LocalGet(synth_idx));
            function.instruction(&Instruction::LocalGet(*len_local));
            function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            // memory.copy(new_base + 4, original_ptr, len)
            function.instruction(&Instruction::LocalGet(synth_idx));
            function.instruction(&Instruction::I32Const(4));
            function.instruction(&Instruction::I32Add);
            function.instruction(&Instruction::LocalGet(*ptr_local));
            function.instruction(&Instruction::LocalGet(*len_local));
            function.instruction(&Instruction::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            });
            // synthetic local = new_base + 4 (points to data, length at ptr-4)
            function.instruction(&Instruction::LocalGet(synth_idx));
            function.instruction(&Instruction::I32Const(4));
            function.instruction(&Instruction::I32Add);
            function.instruction(&Instruction::LocalSet(synth_idx));
        }

        // Track type info for record variables
        let mut locals_types: HashMap<String, RecordTypeInfo> = HashMap::new();

        if let Some(ref body) = func.body {
            let stmts = &body.statements;
            if !stmts.is_empty() {
                // Compile all but last statement normally
                for stmt in &stmts[..stmts.len() - 1] {
                    self.compile_statement_with_locals(
                        &mut function,
                        stmt,
                        &locals,
                        &mut locals_types,
                    )?;
                }
                // Last statement: if it's an expression, leave value on stack for return
                let last = &stmts[stmts.len() - 1];
                match last {
                    Statement::CompoundAssign { name, op, value } => {
                        // Compile as: local.get + value + op + local.set
                        if let Some(&idx) = locals.get(&name.name) {
                            function.instruction(&Instruction::LocalGet(idx));
                        }
                        self.compile_expr_with_locals(&mut function, value, &locals, &locals_types)?;
                        match op {
                            BinOp::Add => function.instruction(&Instruction::I32Add),
                            BinOp::Sub => function.instruction(&Instruction::I32Sub),
                            _ => function.instruction(&Instruction::I32Add),
                        };
                        if let Some(&idx) = locals.get(&name.name) {
                            function.instruction(&Instruction::LocalSet(idx));
                        }
                        function.instruction(&Instruction::I32Const(0));
                    }
                    Statement::Expr(expr) => {
                        // Compile the expression
                        self.compile_expr_with_locals(&mut function, expr, &locals, &locals_types)?;

                        // For async functions with --wasip3: call task.return then return 0 (DONE)
                        if func.is_async && self.options.wasip3 {
                            if let Some(ref result_ty) = func.result {
                                let result_valtype = self.ty_to_valtype(result_ty)?;
                                let task_return_idx =
                                    self.ensure_task_return_import(&[result_valtype]);
                                function.instruction(&Instruction::Call(task_return_idx));
                            }
                            function.instruction(&Instruction::I32Const(0)); // status: DONE
                        }
                        // For sync (or async without wasip3): value is already on stack
                    }
                    Statement::Return(Some(expr)) => {
                        self.compile_expr_with_locals(&mut function, expr, &locals, &locals_types)?;

                        if func.is_async && self.options.wasip3 {
                            if let Some(ref result_ty) = func.result {
                                let result_valtype = self.ty_to_valtype(result_ty)?;
                                let task_return_idx =
                                    self.ensure_task_return_import(&[result_valtype]);
                                function.instruction(&Instruction::Call(task_return_idx));
                            }
                            function.instruction(&Instruction::I32Const(0)); // status: DONE
                            function.instruction(&Instruction::Return);
                        } else {
                            function.instruction(&Instruction::Return);
                        }
                    }
                    Statement::Return(None) => {
                        if func.is_async && self.options.wasip3 {
                            let task_return_idx = self.ensure_task_return_import(&[]);
                            function.instruction(&Instruction::Call(task_return_idx));
                            function.instruction(&Instruction::I32Const(0)); // status: DONE
                        }
                        function.instruction(&Instruction::Return);
                    }
                    Statement::Let { name, value } => {
                        // Track record type info
                        if let Expr::RecordLiteral { fields, .. } = value {
                            let field_info: Vec<_> = fields
                                .iter()
                                .enumerate()
                                .map(|(i, (field_name, _))| (field_name.name.clone(), i * 4))
                                .collect();
                            locals_types.insert(
                                name.name.clone(),
                                RecordTypeInfo::from_fields(&field_info),
                            );
                        }
                        // Compile and store, but also leave copy on stack if func returns value
                        self.compile_expr_with_locals(
                            &mut function,
                            value,
                            &locals,
                            &locals_types,
                        )?;
                        if let Some(&idx) = locals.get(&name.name) {
                            function.instruction(&Instruction::LocalSet(idx));
                        }
                        // For async with wasip3: push 0 (status: DONE); for sync: push default if has result
                        if (func.is_async && self.options.wasip3) || func.result.is_some() {
                            function.instruction(&Instruction::I32Const(0));
                        }
                    }
                    Statement::Assign { name, value } => {
                        // Compile value and store to existing local
                        self.compile_expr_with_locals(
                            &mut function,
                            value,
                            &locals,
                            &locals_types,
                        )?;
                        if let Some(&idx) = locals.get(&name.name) {
                            function.instruction(&Instruction::LocalSet(idx));
                        }
                        // For async with wasip3: push 0 (status: DONE); for sync: push default if has result
                        if (func.is_async && self.options.wasip3) || func.result.is_some() {
                            function.instruction(&Instruction::I32Const(0));
                        }
                    }
                    Statement::Break { .. } | Statement::Continue { .. } => {
                        // These only make sense inside while loops; handled there
                        if (func.is_async && self.options.wasip3) || func.result.is_some() {
                            function.instruction(&Instruction::I32Const(0));
                        }
                    }
                    Statement::SharedLet { .. } => {
                        if (func.is_async && self.options.wasip3) || func.result.is_some() {
                            function.instruction(&Instruction::I32Const(0));
                        }
                    }
                }
            } else if (func.is_async && self.options.wasip3) || func.result.is_some() {
                // Empty body - push default value (0 for async status or sync result)
                function.instruction(&Instruction::I32Const(0));
            }
        }

        // If this async function has await points, register a callback export
        if needs_callback {
            let num_locals = let_count + 4; // locals + extra temp
            let _state_offset = self.alloc_async_state(num_locals);
            let _callback_idx =
                self.emit_async_callback(&func.name.name, func, _state_offset, num_locals)?;
            // The entry function compilation already handles await with blocking wait
            // The callback will be invoked for non-blocking resumption (future work)
        }

        // Ensure function ends properly
        function.instruction(&Instruction::End);

        Ok(function)
    }

    fn compile_statement_with_locals(
        &mut self,
        function: &mut Function,
        stmt: &Statement,
        locals: &HashMap<String, u32>,
        locals_types: &mut HashMap<String, RecordTypeInfo>,
    ) -> Result<(), CompileError> {
        match stmt {
            Statement::Expr(expr) => {
                self.compile_expr_with_locals(function, expr, locals, locals_types)?;
                // Drop the result if expression produces a value
                function.instruction(&Instruction::Drop);
            }
            Statement::Let { name, value } => {
                // Track record type info for field access
                if let Expr::RecordLiteral { fields, .. } = value {
                    let field_info: Vec<_> = fields
                        .iter()
                        .enumerate()
                        .map(|(i, (field_name, _))| (field_name.name.clone(), i * 4))
                        .collect();
                    locals_types
                        .insert(name.name.clone(), RecordTypeInfo::from_fields(&field_info));
                }
                // Track closure info for Lambda assignments
                if let Expr::Lambda { params, body, .. } = value {
                    // Find captures for this lambda
                    let mut bound: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
                    for p in params {
                        bound.insert(p.name.clone());
                    }
                    let free_vars = kettu_parser::capture::find_free_variables(body, &bound);
                    let captures: Vec<String> = free_vars
                        .iter()
                        .filter(|name| locals.contains_key(*name))
                        .cloned()
                        .collect();
                    if !captures.is_empty() {
                        self.closure_info.insert(name.name.clone(), captures);
                    }
                }
                // Compile value and store in local
                self.compile_expr_with_locals(function, value, locals, locals_types)?;
                if let Some(&idx) = locals.get(&name.name) {
                    function.instruction(&Instruction::LocalSet(idx));
                }
            }
            Statement::Return(expr) => {
                if let Some(expr) = expr {
                    self.compile_expr_with_locals(function, expr, locals, locals_types)?;
                }
                function.instruction(&Instruction::Return);
            }
            Statement::Assign { name, value } => {
                // Compile value and store to existing local
                self.compile_expr_with_locals(function, value, locals, locals_types)?;
                if let Some(&idx) = locals.get(&name.name) {
                    function.instruction(&Instruction::LocalSet(idx));
                }
            }
            Statement::CompoundAssign { name, op, value } => {
                // local.get + value + binop + local.set
                if let Some(&idx) = locals.get(&name.name) {
                    function.instruction(&Instruction::LocalGet(idx));
                }
                self.compile_expr_with_locals(function, value, locals, locals_types)?;
                match op {
                    BinOp::Add => { function.instruction(&Instruction::I32Add); }
                    BinOp::Sub => { function.instruction(&Instruction::I32Sub); }
                    _ => { function.instruction(&Instruction::I32Add); }
                }
                if let Some(&idx) = locals.get(&name.name) {
                    function.instruction(&Instruction::LocalSet(idx));
                }
            }
            Statement::Break { .. } => {
                if let Some(depth) = self.loop_break_depth {
                    function.instruction(&Instruction::Br(depth));
                }
            }
            Statement::Continue { .. } => {
                if let Some(depth) = self.loop_continue_depth {
                    function.instruction(&Instruction::Br(depth));
                }
            }
            Statement::SharedLet { name, initial_value } => {
                let offset = self.string_offset;
                self.string_offset += 4;
                // Store offset in shared memory and initialize
                function.instruction(&Instruction::I32Const(offset as i32));
                self.compile_expr_with_locals(function, initial_value, locals, locals_types)?;
                function.instruction(&Instruction::I32AtomicStore(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                // Store the memory offset in the local variable for later reference
                if let Some(&idx) = locals.get(&name.name) {
                    function.instruction(&Instruction::I32Const(offset as i32));
                    function.instruction(&Instruction::LocalSet(idx));
                }
                self.shared_locals.insert(name.name.clone());
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    fn compile_statement(
        &mut self,
        function: &mut Function,
        stmt: &Statement,
    ) -> Result<(), CompileError> {
        match stmt {
            Statement::CompoundAssign { .. } => {
                function.instruction(&Instruction::I32Const(0));
            }
            Statement::Expr(expr) => {
                self.compile_expr(function, expr)?;
                // Drop the result if expression produces a value
                function.instruction(&Instruction::Drop);
            }
            Statement::Let { name: _, value } => {
                // For now, just compile the value (locals not fully supported yet)
                self.compile_expr(function, value)?;
                function.instruction(&Instruction::Drop);
            }
            Statement::Return(expr) => {
                if let Some(expr) = expr {
                    self.compile_expr(function, expr)?;
                }
                function.instruction(&Instruction::Return);
            }
            Statement::Assign { name: _, value } => {
                // For now, just compile the value (locals not fully supported yet)
                self.compile_expr(function, value)?;
                function.instruction(&Instruction::Drop);
            }
            Statement::Break { .. } | Statement::Continue { .. } => {
                // These only make sense inside while loops; handled there
            }
            Statement::SharedLet { .. } => {}
        }
        Ok(())
    }

    #[allow(dead_code)]
    fn compile_expr(&mut self, function: &mut Function, expr: &Expr) -> Result<(), CompileError> {
        match expr {
            Expr::Integer(n, _) => {
                if *n >= i32::MIN as i64 && *n <= i32::MAX as i64 {
                    function.instruction(&Instruction::I32Const(*n as i32));
                } else {
                    function.instruction(&Instruction::I64Const(*n));
                }
            }
            Expr::Bool(b, _) => {
                function.instruction(&Instruction::I32Const(if *b { 1 } else { 0 }));
            }
            Expr::String(s, _) => {
                // Register string literal and emit pointer to it
                let (offset, _len) = self.register_string(s);
                function.instruction(&Instruction::I32Const(offset as i32));
            }
            Expr::InterpolatedString(_, _) => {
                // InterpolatedString requires compile_expr_with_locals - return empty string
                let (offset, _len) = self.register_string("");
                function.instruction(&Instruction::I32Const(offset as i32));
            }
            Expr::Ident(id) => {
                // Variable reference - for now, check if it's a function call without args
                if let Some(&(_, func_idx, _)) = self.functions.get(&id.name) {
                    function.instruction(&Instruction::Call(func_idx));
                } else {
                    // Local variable - not yet implemented
                    function.instruction(&Instruction::I32Const(0));
                }
            }
            Expr::Call {
                func: callee, args, ..
            } => {
                // Compile arguments first
                for arg in args {
                    self.compile_expr(function, arg)?;
                }

                // Get function to call
                if let Expr::Ident(id) = callee.as_ref() {
                    if let Some(&(_, func_idx, _)) = self.functions.get(&id.name) {
                        function.instruction(&Instruction::Call(func_idx));
                    } else {
                        // Unknown function - drop all arguments and push placeholder
                        // This allows for runtime imports that aren't resolved at compile time
                        for _ in args {
                            function.instruction(&Instruction::Drop);
                        }
                        function.instruction(&Instruction::I32Const(0));
                    }
                } else if let Expr::Field {
                    expr: receiver,
                    field,
                    ..
                } = callee.as_ref()
                {
                    // Qualified call: interface.function() or record.method()
                    if let Expr::Ident(interface_id) = receiver.as_ref() {
                        let interface_name = &interface_id.name;
                        let func_name = &field.name;

                        // Check if this is an imported interface call
                        if let Some((_, func_map)) = self.imported_interfaces.get(interface_name) {
                            if let Some(&func_idx) = func_map.get(func_name) {
                                function.instruction(&Instruction::Call(func_idx));
                            } else {
                                // Unknown function in interface - placeholder
                                for _ in args {
                                    function.instruction(&Instruction::Drop);
                                }
                                function.instruction(&Instruction::I32Const(0));
                            }
                        } else {
                            // Not an imported interface - try qualified function name
                            let qualified_name = format!("{}.{}", interface_name, func_name);
                            if let Some(&(_, func_idx, _)) = self.functions.get(&qualified_name) {
                                function.instruction(&Instruction::Call(func_idx));
                            } else {
                                // Fall back to indirect call
                                self.compile_expr(function, callee)?;
                                let param_types: Vec<ValType> =
                                    args.iter().map(|_| ValType::I32).collect();
                                let result_types = vec![ValType::I32];
                                let type_idx = self
                                    .types
                                    .iter()
                                    .position(|(p, r)| p == &param_types && r == &result_types)
                                    .map(|i| i as u32)
                                    .unwrap_or_else(|| {
                                        let idx = self.types.len() as u32;
                                        self.types.push((param_types, result_types));
                                        idx
                                    });
                                function.instruction(&Instruction::CallIndirect {
                                    type_index: type_idx,
                                    table_index: 0,
                                });
                            }
                        }
                    } else {
                        // Complex receiver - fall back to indirect call
                        self.compile_expr(function, callee)?;
                        let param_types: Vec<ValType> = args.iter().map(|_| ValType::I32).collect();
                        let result_types = vec![ValType::I32];
                        let type_idx = self
                            .types
                            .iter()
                            .position(|(p, r)| p == &param_types && r == &result_types)
                            .map(|i| i as u32)
                            .unwrap_or_else(|| {
                                let idx = self.types.len() as u32;
                                self.types.push((param_types, result_types));
                                idx
                            });
                        function.instruction(&Instruction::CallIndirect {
                            type_index: type_idx,
                            table_index: 0,
                        });
                    }
                } else {
                    // Indirect call - callee is an expression that evaluates to a table index
                    // Push the table index onto the stack
                    self.compile_expr(function, callee)?;

                    // Use call_indirect with the type signature matching the arity
                    // For now, assume all lambdas take N i32 args and return i32
                    let param_types: Vec<ValType> = args.iter().map(|_| ValType::I32).collect();
                    let result_types = vec![ValType::I32];

                    // Find or create the type index
                    let type_idx = self
                        .types
                        .iter()
                        .position(|(p, r)| p == &param_types && r == &result_types)
                        .map(|i| i as u32)
                        .unwrap_or_else(|| {
                            let idx = self.types.len() as u32;
                            self.types.push((param_types, result_types));
                            idx
                        });

                    // call_indirect: table 0, type index
                    function.instruction(&Instruction::CallIndirect {
                        type_index: type_idx,
                        table_index: 0,
                    });
                }
            }
            Expr::Field { expr, field, .. } => {
                // Compile the record expression (pushes pointer)
                self.compile_expr(function, expr)?;

                // Calculate field offset - for now, assume 4 bytes per field
                // Try to find field index from the expression if it's a RecordLiteral
                let offset = if let Expr::RecordLiteral { fields, .. } = expr.as_ref() {
                    fields
                        .iter()
                        .position(|(name, _)| name.name == field.name)
                        .map(|i| (i * 4) as u64)
                        .unwrap_or(0)
                } else {
                    // For non-literal records, we'd need type info
                    // For now, assume field index is encoded somehow
                    0
                };

                // Load i32 at offset
                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Expr::Binary { lhs, op, rhs, .. } => {
                use kettu_parser::BinOp;
                match op {
                    // Short-circuit &&: if lhs is false, result is false; else result is rhs
                    BinOp::And => {
                        self.compile_expr(function, lhs)?;
                        function.instruction(&Instruction::If(wasm_encoder::BlockType::Result(
                            wasm_encoder::ValType::I32,
                        )));
                        // LHS was true, evaluate RHS
                        self.compile_expr(function, rhs)?;
                        function.instruction(&Instruction::Else);
                        // LHS was false, result is 0
                        function.instruction(&Instruction::I32Const(0));
                        function.instruction(&Instruction::End);
                    }
                    // Short-circuit ||: if lhs is true, result is true; else result is rhs
                    BinOp::Or => {
                        self.compile_expr(function, lhs)?;
                        function.instruction(&Instruction::If(wasm_encoder::BlockType::Result(
                            wasm_encoder::ValType::I32,
                        )));
                        // LHS was true, result is 1
                        function.instruction(&Instruction::I32Const(1));
                        function.instruction(&Instruction::Else);
                        // LHS was false, evaluate RHS
                        self.compile_expr(function, rhs)?;
                        function.instruction(&Instruction::End);
                    }
                    // Non-short-circuit operators: evaluate both sides
                    _ => {
                        self.compile_expr(function, lhs)?;
                        self.compile_expr(function, rhs)?;
                        match op {
                            BinOp::Add => {
                                // Check if this is string concatenation
                                if Self::is_string_expr(lhs) {
                                    let concat_idx = self.ensure_str_concat_func();
                                    function.instruction(&Instruction::Call(concat_idx))
                                } else {
                                    function.instruction(&Instruction::I32Add)
                                }
                            }
                            BinOp::Sub => function.instruction(&Instruction::I32Sub),
                            BinOp::Mul => function.instruction(&Instruction::I32Mul),
                            BinOp::Div => function.instruction(&Instruction::I32DivS),
                            BinOp::Eq => function.instruction(&Instruction::I32Eq),
                            BinOp::Ne => function.instruction(&Instruction::I32Ne),
                            BinOp::Lt => function.instruction(&Instruction::I32LtS),
                            BinOp::Le => function.instruction(&Instruction::I32LeS),
                            BinOp::Gt => function.instruction(&Instruction::I32GtS),
                            BinOp::Ge => function.instruction(&Instruction::I32GeS),
                            BinOp::And | BinOp::Or => unreachable!(),
                        };
                    }
                }
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                // Compile condition
                self.compile_expr(function, cond)?;
                // If expression returns i32 (bool is i32 in WASM)
                function.instruction(&Instruction::If(wasm_encoder::BlockType::Result(
                    wasm_encoder::ValType::I32,
                )));

                // Helper to compile a branch - last statement should leave value on stack
                fn compile_branch(
                    compiler: &mut ModuleCompiler,
                    function: &mut Function,
                    stmts: &[Statement],
                ) -> Result<(), CompileError> {
                    if stmts.is_empty() {
                        // Empty branch - push 0
                        function.instruction(&Instruction::I32Const(0));
                        return Ok(());
                    }
                    // Compile all but last statement normally (dropping values)
                    for stmt in &stmts[..stmts.len() - 1] {
                        compiler.compile_statement(function, stmt)?;
                    }
                    // Last statement: compile without dropping
                    match &stmts[stmts.len() - 1] {
                        Statement::Expr(expr) => {
                            compiler.compile_expr(function, expr)?;
                            // Don't drop - leave value on stack
                        }
                        Statement::Return(Some(expr)) => {
                            compiler.compile_expr(function, expr)?;
                            function.instruction(&Instruction::Return);
                        }
                        Statement::Return(None) => {
                            function.instruction(&Instruction::I32Const(0));
                            function.instruction(&Instruction::Return);
                        }
                        Statement::Let { value, .. } => {
                            compiler.compile_expr(function, value)?;
                            // Leave value on stack (side effect: doesn't store in local)
                        }
                        Statement::Assign { value, .. }
                        | Statement::CompoundAssign { value, .. } => {
                            compiler.compile_expr(function, value)?;
                            // Leave value on stack
                        }
                        Statement::Break { .. } | Statement::Continue { .. }
                        | Statement::SharedLet { .. } => {
                            // These shouldn't appear in if branches; push 0 for stack balance
                            function.instruction(&Instruction::I32Const(0));
                        }
                    }
                    Ok(())
                }

                compile_branch(self, function, then_branch)?;

                if let Some(else_stmts) = else_branch {
                    function.instruction(&Instruction::Else);
                    compile_branch(self, function, else_stmts)?;
                } else {
                    // No else branch - provide default value 0
                    function.instruction(&Instruction::Else);
                    function.instruction(&Instruction::I32Const(0));
                }
                function.instruction(&Instruction::End);
            }
            Expr::Assert(cond, _) => {
                // Compile condition
                self.compile_expr(function, cond)?;
                // If false, trap with unreachable
                function.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
                // Condition was true - do nothing
                function.instruction(&Instruction::Else);
                // Condition was false - trap
                function.instruction(&Instruction::Unreachable);
                function.instruction(&Instruction::End);
                // Leave true on stack (assert passed)
                function.instruction(&Instruction::I32Const(1));
            }
            Expr::Not(expr, _) => {
                // Compile the expression
                self.compile_expr(function, expr)?;
                // Negate: 0 -> 1, non-zero -> 0
                function.instruction(&Instruction::I32Eqz);
            }
            Expr::Neg(expr, _) => {
                // Unary negation: 0 - expr
                function.instruction(&Instruction::I32Const(0));
                self.compile_expr(function, expr)?;
                function.instruction(&Instruction::I32Sub);
            }
            Expr::StrLen(expr, _) => {
                // Compile string expression (pushes pointer)
                self.compile_expr(function, expr)?;
                // Read length from ptr - 4
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Sub);
                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Expr::StrEq(a, b, _) => {
                // Compare two strings by content using builtin str_eq function
                let str_eq_idx = self.ensure_str_eq_func();
                self.compile_expr(function, a)?;
                self.compile_expr(function, b)?;
                function.instruction(&Instruction::Call(str_eq_idx));
            }
            Expr::ListLen(expr, _) => {
                // Compile list expression (pushes pointer)
                self.compile_expr(function, expr)?;
                // Read length from offset 0 (list layout: [len:i32][elem0][elem1]...)
                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Expr::ListSet(_, _, _, _) => {
                // ListSet without locals - just push 0 (requires compile_expr_with_locals)
                function.instruction(&Instruction::I32Const(0));
            }
            Expr::Slice { .. } => {
                // Slice without locals - just push 0 (requires compile_expr_with_locals)
                function.instruction(&Instruction::I32Const(0));
            }
            Expr::ListPush(_, _, _) => {
                // ListPush without locals - just push 0 (requires compile_expr_with_locals)
                function.instruction(&Instruction::I32Const(0));
            }
            Expr::Lambda {
                captures,
                params,
                body,
                ..
            } => {
                // Emit lambda as function and return table index
                let table_idx = self.emit_lambda(captures, params, body)?;
                function.instruction(&Instruction::I32Const(table_idx as i32));
            }
            Expr::Map { .. } => {
                // Map requires compile_expr_with_locals - return 0 for now
                function.instruction(&Instruction::I32Const(0));
            }
            Expr::Filter { .. } => {
                // Filter requires compile_expr_with_locals - return 0 for now
                function.instruction(&Instruction::I32Const(0));
            }
            Expr::Reduce { .. } => {
                // Reduce requires compile_expr_with_locals - return 0 for now
                function.instruction(&Instruction::I32Const(0));
            }
            Expr::RecordLiteral { fields, .. } => {
                // Allocate space for record (4 bytes per field for i32 values)
                let size = (fields.len() * 4) as i32;
                let alloc_idx = self.alloc_func_idx.unwrap();

                // Call alloc(size)
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::I32Const(size));
                function.instruction(&Instruction::Call(alloc_idx));

                // Store each field value at offset
                for (i, (_, expr)) in fields.iter().enumerate() {
                    // Duplicate pointer
                    function.instruction(&Instruction::LocalTee(0)); // Assume local 0 for temp
                    // Compile field value
                    self.compile_expr(function, expr)?;
                    // Store at offset
                    function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                        offset: (i * 4) as u64,
                        align: 2,
                        memory_index: 0,
                    }));
                }
                // Final push of pointer is already on stack from last LocalTee
            }
            Expr::VariantLiteral {
                case_name, payload, ..
            } => {
                // Variant layout: [discriminant: i32] [payload: i32 (optional)]
                // For now, discriminant is hash of case name (simple approach)
                let discriminant = case_name
                    .name
                    .bytes()
                    .fold(0u32, |acc, b| acc.wrapping_add(b as u32));

                if payload.is_some() {
                    // Allocate 8 bytes: 4 for discriminant, 4 for payload
                    let alloc_idx = self.alloc_func_idx.unwrap();
                    function.instruction(&Instruction::I32Const(0));
                    function.instruction(&Instruction::I32Const(0));
                    function.instruction(&Instruction::I32Const(0));
                    function.instruction(&Instruction::I32Const(8));
                    function.instruction(&Instruction::Call(alloc_idx));

                    // Store discriminant at offset 0
                    function.instruction(&Instruction::LocalTee(0));
                    function.instruction(&Instruction::I32Const(discriminant as i32));
                    function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                        offset: 0,
                        align: 2,
                        memory_index: 0,
                    }));

                    // Store payload at offset 4
                    function.instruction(&Instruction::LocalGet(0));
                    self.compile_expr(function, payload.as_ref().unwrap())?;
                    function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                        offset: 4,
                        align: 2,
                        memory_index: 0,
                    }));

                    // Push pointer back
                    function.instruction(&Instruction::LocalGet(0));
                } else {
                    // No payload - just push discriminant as value
                    function.instruction(&Instruction::I32Const(discriminant as i32));
                }
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                // For match without locals - compile as if-else chain on discriminant
                // Compile scrutinee (should leave value/pointer on stack)
                self.compile_expr(function, scrutinee)?;

                // For now, just execute first arm body as placeholder
                // Full match needs type info for proper discriminant extraction
                if let Some(arm) = arms.first() {
                    if let Some(Statement::Expr(body_expr)) = arm.body.first() {
                        // Drop the scrutinee value
                        function.instruction(&Instruction::Drop);
                        self.compile_expr(function, body_expr)?;
                    } else {
                        function.instruction(&Instruction::I32Const(0));
                    }
                } else {
                    function.instruction(&Instruction::I32Const(0));
                }
            }
            Expr::While {
                condition, body, ..
            } => {
                // WASM while loop pattern:
                // (block $break
                //   (loop $continue
                //     br_if $break (i32.eqz condition)  ;; exit if false
                //     body
                //     br $continue  ;; loop back
                //   )
                // )
                // While loops return unit (i32 0)

                // Outer block for break target
                function.instruction(&Instruction::Block(wasm_encoder::BlockType::Empty));
                // Inner loop for continue
                function.instruction(&Instruction::Loop(wasm_encoder::BlockType::Empty));

                // Evaluate condition
                self.compile_expr(function, condition)?;
                // If condition is false (eqz), break out
                function.instruction(&Instruction::I32Eqz);
                function.instruction(&Instruction::BrIf(1)); // break to outer block

                // Compile body
                for stmt in body {
                    if let Statement::Expr(e) = stmt {
                        self.compile_expr(function, e)?;
                        // Drop result of expression (while body is statement)
                        function.instruction(&Instruction::Drop);
                    }
                }

                // Branch back to loop start
                function.instruction(&Instruction::Br(0));
                function.instruction(&Instruction::End); // end loop
                function.instruction(&Instruction::End); // end block

                // While returns unit (push 0)
                function.instruction(&Instruction::I32Const(0));
            }
            Expr::Range { .. } => {
                // Range expressions are only valid inside for loops
                // Push dummy value
                function.instruction(&Instruction::I32Const(0));
            }
            Expr::For { .. } => {
                // For loop without locals - just push 0
                // Full for loop support requires locals (use compile_expr_with_locals)
                function.instruction(&Instruction::I32Const(0));
            }
            Expr::ListLiteral { .. } => {
                // List literal without locals - full support requires compile_expr_with_locals
                function.instruction(&Instruction::I32Const(0));
            }
            Expr::Index { .. } => {
                // Index without locals - full support requires compile_expr_with_locals
                function.instruction(&Instruction::I32Const(0));
            }
            Expr::ForEach { .. } => {
                // For-each without locals - just push 0
                // Full for-each support requires locals (use compile_expr_with_locals)
                function.instruction(&Instruction::I32Const(0));
            }
            Expr::OptionalChain { .. } => {
                // Optional chaining requires compile_expr_with_locals for full support
                function.instruction(&Instruction::I32Const(0));
            }
            Expr::Try { .. } => {
                // Try operator requires compile_expr_with_locals for full support
                function.instruction(&Instruction::I32Const(0));
            }
            Expr::Await { .. } => {
                // Await requires async runtime support - placeholder for now
                function.instruction(&Instruction::I32Const(0));
            }
            Expr::AtomicLoad { .. } | Expr::AtomicStore { .. } | Expr::AtomicAdd { .. }
            | Expr::AtomicSub { .. } | Expr::AtomicCmpxchg { .. }
            | Expr::AtomicWait { .. } | Expr::AtomicNotify { .. }
            | Expr::Spawn { .. } | Expr::AtomicBlock { .. }
            | Expr::ThreadJoin { .. }
            | Expr::SimdOp { .. }
            | Expr::SimdForEach { .. } => {
                function.instruction(&Instruction::I32Const(0));
            }
        }
        Ok(())
    }

    fn compile_expr_with_locals(
        &mut self,
        function: &mut Function,
        expr: &Expr,
        locals: &HashMap<String, u32>,
        locals_types: &HashMap<String, RecordTypeInfo>,
    ) -> Result<(), CompileError> {
        match expr {
            Expr::Integer(n, _) => {
                if *n >= i32::MIN as i64 && *n <= i32::MAX as i64 {
                    function.instruction(&Instruction::I32Const(*n as i32));
                } else {
                    function.instruction(&Instruction::I64Const(*n));
                }
            }
            Expr::Bool(b, _) => {
                function.instruction(&Instruction::I32Const(if *b { 1 } else { 0 }));
            }
            Expr::String(s, _) => {
                // Register string literal and emit pointer to it
                let (offset, _len) = self.register_string(s);
                function.instruction(&Instruction::I32Const(offset as i32));
            }
            Expr::InterpolatedString(parts, _) => {
                // Build string by concatenating all parts using the built-in str_concat
                if parts.is_empty() {
                    let (offset, _len) = self.register_string("");
                    function.instruction(&Instruction::I32Const(offset as i32));
                } else if parts.len() == 1 {
                    match &parts[0] {
                        StringPart::Literal(s) => {
                            let (offset, _len) = self.register_string(s);
                            function.instruction(&Instruction::I32Const(offset as i32));
                        }
                        StringPart::Expr(expr) => {
                            self.compile_expr_with_locals(function, expr, locals, locals_types)?;
                        }
                    }
                } else {
                    // Multiple parts: ensure str_concat is available
                    let concat_idx = self.ensure_str_concat_func();
                    let mut first = true;
                    for part in parts {
                        match part {
                            StringPart::Literal(s) => {
                                let (offset, _len) = self.register_string(s);
                                function.instruction(&Instruction::I32Const(offset as i32));
                            }
                            StringPart::Expr(expr) => {
                                self.compile_expr_with_locals(
                                    function,
                                    expr,
                                    locals,
                                    locals_types,
                                )?;
                            }
                        }
                        if !first {
                            function.instruction(&Instruction::Call(concat_idx));
                        }
                        first = false;
                    }
                }
            }
            Expr::Lambda {
                captures: _,
                params,
                body,
                ..
            } => {
                // Find captures for this lambda
                let mut bound: std::collections::HashSet<String> = std::collections::HashSet::new();
                for p in params {
                    bound.insert(p.name.clone());
                }
                let free_vars = kettu_parser::capture::find_free_variables(body, &bound);
                let actual_captures: Vec<String> = free_vars
                    .iter()
                    .filter(|name| locals.contains_key(*name))
                    .cloned()
                    .collect();

                // Build capture Ids for emit_lambda
                let capture_ids: Vec<kettu_parser::Id> = actual_captures
                    .iter()
                    .map(|name| kettu_parser::Id {
                        name: name.clone(),
                        span: 0..0,
                    })
                    .collect();

                // Emit lambda as function (with captures as hidden params)
                let table_idx = self.emit_lambda(&capture_ids, params, body)?;

                // Always allocate closure cell: [table_idx, capture_count, cap1, cap2, ...]
                let cell_size = (2 + actual_captures.len()) * 4; // i32 each
                let alloc_idx = self.alloc_func_idx.unwrap();

                // Allocate closure cell
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::I32Const(cell_size as i32));
                function.instruction(&Instruction::Call(alloc_idx));

                // Need temp local for cell ptr - use next available
                let temp_base = locals.len() as u32;
                let cell_ptr_local = temp_base;
                function.instruction(&Instruction::LocalTee(cell_ptr_local));

                // Store table_idx at offset 0
                function.instruction(&Instruction::I32Const(table_idx as i32));
                function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));

                // Store capture count at offset 4
                function.instruction(&Instruction::LocalGet(cell_ptr_local));
                function.instruction(&Instruction::I32Const(actual_captures.len() as i32));
                function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));

                // Store each capture value
                for (i, cap_name) in actual_captures.iter().enumerate() {
                    let cap_local = locals.get(cap_name).unwrap();
                    function.instruction(&Instruction::LocalGet(cell_ptr_local));
                    function.instruction(&Instruction::LocalGet(*cap_local));
                    function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                        offset: ((2 + i) * 4) as u64,
                        align: 2,
                        memory_index: 0,
                    }));
                }

                // Leave cell pointer on stack as the closure value
                function.instruction(&Instruction::LocalGet(cell_ptr_local));
            }
            Expr::Map { list, lambda, .. } => {
                // Map: allocate new list, loop over source, inline lambda body
                let alloc_idx = self.alloc_func_idx.unwrap();

                // Get temp locals: src_ptr, len, dest_ptr, i, elem
                let temp_base = locals.len() as u32;
                let src_ptr_local = temp_base;
                let len_local = temp_base + 1;
                let dest_ptr_local = temp_base + 2;
                let i_local = temp_base + 3;
                let elem_local = temp_base + 4;

                // Compile source list and store pointer
                self.compile_expr_with_locals(function, list, locals, locals_types)?;
                function.instruction(&Instruction::LocalSet(src_ptr_local));

                // Load source length
                function.instruction(&Instruction::LocalGet(src_ptr_local));
                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                function.instruction(&Instruction::LocalSet(len_local));

                // Allocate dest list: 4 (length) + len * 4
                // Push CABI args: (0, 0, 0, size) where size = 4 + len * 4
                function.instruction(&Instruction::I32Const(0)); // ptr (unused)
                function.instruction(&Instruction::I32Const(0)); // old_size (unused)
                function.instruction(&Instruction::I32Const(0)); // align (unused)
                function.instruction(&Instruction::LocalGet(len_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Mul);
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::Call(alloc_idx));
                function.instruction(&Instruction::LocalSet(dest_ptr_local));

                // Store length at dest[0]
                function.instruction(&Instruction::LocalGet(dest_ptr_local));
                function.instruction(&Instruction::LocalGet(len_local));
                function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));

                // Initialize i = 0
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::LocalSet(i_local));

                // Transform loop
                function.instruction(&Instruction::Block(wasm_encoder::BlockType::Empty));
                function.instruction(&Instruction::Loop(wasm_encoder::BlockType::Empty));

                // Check: i >= len => break
                function.instruction(&Instruction::LocalGet(i_local));
                function.instruction(&Instruction::LocalGet(len_local));
                function.instruction(&Instruction::I32GeS);
                function.instruction(&Instruction::BrIf(1));

                // Load src[4 + i*4] into elem_local
                function.instruction(&Instruction::LocalGet(src_ptr_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalGet(i_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Mul);
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                function.instruction(&Instruction::LocalSet(elem_local));

                // Prepare store address: dest[4 + i*4]
                function.instruction(&Instruction::LocalGet(dest_ptr_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalGet(i_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Mul);
                function.instruction(&Instruction::I32Add);

                // Compile lambda body with param bound to elem_local
                if let Expr::Lambda { params, body, .. } = lambda.as_ref() {
                    if !params.is_empty() {
                        // Bind the lambda param to elem_local
                        let mut inner_locals = locals.clone();
                        inner_locals.insert(params[0].name.clone(), elem_local);
                        self.compile_expr_with_locals(function, body, &inner_locals, locals_types)?;
                    } else {
                        // No params - just compile body (identity)
                        self.compile_expr_with_locals(function, body, locals, locals_types)?;
                    }
                } else if let Expr::Ident(id) = lambda.as_ref() {
                    // Function variable - lookup and call_indirect
                    if let Some(&local_idx) = locals.get(&id.name) {
                        // Push argument (element)
                        function.instruction(&Instruction::LocalGet(elem_local));
                        // Load table_idx from closure cell offset 0
                        function.instruction(&Instruction::LocalGet(local_idx));
                        function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                            offset: 0,
                            align: 2,
                            memory_index: 0,
                        }));
                        // call_indirect with (i32) -> i32 type
                        let type_idx = self.get_or_create_type(&[ValType::I32], &[ValType::I32]);
                        function.instruction(&Instruction::CallIndirect {
                            type_index: type_idx,
                            table_index: 0,
                        });
                    } else {
                        // Unknown - identity
                        function.instruction(&Instruction::LocalGet(elem_local));
                    }
                } else {
                    // Expression that evaluates to function reference
                    function.instruction(&Instruction::LocalGet(elem_local));
                    self.compile_expr_with_locals(function, lambda, locals, locals_types)?;
                    let type_idx = self.get_or_create_type(&[ValType::I32], &[ValType::I32]);
                    function.instruction(&Instruction::CallIndirect {
                        type_index: type_idx,
                        table_index: 0,
                    });
                }

                // Store transformed value
                function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));

                // i++
                function.instruction(&Instruction::LocalGet(i_local));
                function.instruction(&Instruction::I32Const(1));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalSet(i_local));

                function.instruction(&Instruction::Br(0));
                function.instruction(&Instruction::End); // end loop
                function.instruction(&Instruction::End); // end block

                // Return dest pointer
                function.instruction(&Instruction::LocalGet(dest_ptr_local));
            }
            Expr::Filter { list, lambda, .. } => {
                // Filter: allocate max-size list, conditionally copy, update length
                let alloc_idx = self.alloc_func_idx.unwrap();

                // Get temp locals: src_ptr, len, dest_ptr, i, j, elem
                let temp_base = locals.len() as u32;
                let src_ptr_local = temp_base;
                let len_local = temp_base + 1;
                let dest_ptr_local = temp_base + 2;
                let i_local = temp_base + 3;
                let j_local = temp_base + 4; // dest index
                let elem_local = temp_base + 5;

                // Compile source list and store pointer
                self.compile_expr_with_locals(function, list, locals, locals_types)?;
                function.instruction(&Instruction::LocalSet(src_ptr_local));

                // Load source length
                function.instruction(&Instruction::LocalGet(src_ptr_local));
                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                function.instruction(&Instruction::LocalSet(len_local));

                // Allocate max-size dest list: 4 (length) + len * 4
                // Push CABI args: (0, 0, 0, size)
                function.instruction(&Instruction::I32Const(0)); // ptr (unused)
                function.instruction(&Instruction::I32Const(0)); // old_size (unused)
                function.instruction(&Instruction::I32Const(0)); // align (unused)
                function.instruction(&Instruction::LocalGet(len_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Mul);
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::Call(alloc_idx));
                function.instruction(&Instruction::LocalSet(dest_ptr_local));

                // Initialize i = 0, j = 0
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::LocalSet(i_local));
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::LocalSet(j_local));

                // Filter loop
                function.instruction(&Instruction::Block(wasm_encoder::BlockType::Empty));
                function.instruction(&Instruction::Loop(wasm_encoder::BlockType::Empty));

                // Check: i >= len => break
                function.instruction(&Instruction::LocalGet(i_local));
                function.instruction(&Instruction::LocalGet(len_local));
                function.instruction(&Instruction::I32GeS);
                function.instruction(&Instruction::BrIf(1));

                // Load src[4 + i*4] into elem_local
                function.instruction(&Instruction::LocalGet(src_ptr_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalGet(i_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Mul);
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                function.instruction(&Instruction::LocalSet(elem_local));

                // Evaluate predicate with param bound to elem_local
                if let Expr::Lambda { params, body, .. } = lambda.as_ref() {
                    if !params.is_empty() {
                        let mut inner_locals = locals.clone();
                        inner_locals.insert(params[0].name.clone(), elem_local);
                        self.compile_expr_with_locals(function, body, &inner_locals, locals_types)?;
                    } else {
                        self.compile_expr_with_locals(function, body, locals, locals_types)?;
                    }
                } else if let Expr::Ident(id) = lambda.as_ref() {
                    // Function variable - call_indirect
                    if let Some(&local_idx) = locals.get(&id.name) {
                        function.instruction(&Instruction::LocalGet(elem_local));
                        // Load table_idx from closure cell offset 0
                        function.instruction(&Instruction::LocalGet(local_idx));
                        function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                            offset: 0,
                            align: 2,
                            memory_index: 0,
                        }));
                        let type_idx = self.get_or_create_type(&[ValType::I32], &[ValType::I32]);
                        function.instruction(&Instruction::CallIndirect {
                            type_index: type_idx,
                            table_index: 0,
                        });
                    } else {
                        function.instruction(&Instruction::I32Const(0)); // false
                    }
                } else {
                    // Expression that evaluates to function reference
                    function.instruction(&Instruction::LocalGet(elem_local));
                    self.compile_expr_with_locals(function, lambda, locals, locals_types)?;
                    let type_idx = self.get_or_create_type(&[ValType::I32], &[ValType::I32]);
                    function.instruction(&Instruction::CallIndirect {
                        type_index: type_idx,
                        table_index: 0,
                    });
                }

                // If predicate is false, skip copy (jump to i++)
                function.instruction(&Instruction::I32Eqz);
                function.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
                // else: copy element to dest[4 + j*4]
                function.instruction(&Instruction::Else);

                function.instruction(&Instruction::LocalGet(dest_ptr_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalGet(j_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Mul);
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalGet(elem_local));
                function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));

                // j++
                function.instruction(&Instruction::LocalGet(j_local));
                function.instruction(&Instruction::I32Const(1));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalSet(j_local));

                function.instruction(&Instruction::End); // end if

                // i++
                function.instruction(&Instruction::LocalGet(i_local));
                function.instruction(&Instruction::I32Const(1));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalSet(i_local));

                function.instruction(&Instruction::Br(0));
                function.instruction(&Instruction::End); // end loop
                function.instruction(&Instruction::End); // end block

                // Store final length (j) at dest[0]
                function.instruction(&Instruction::LocalGet(dest_ptr_local));
                function.instruction(&Instruction::LocalGet(j_local));
                function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));

                // Return dest pointer
                function.instruction(&Instruction::LocalGet(dest_ptr_local));
            }
            Expr::Reduce {
                list, init, lambda, ..
            } => {
                // Reduce: fold list to single value with accumulator
                // Temp locals: src_ptr, len, i, acc
                let temp_base = locals.len() as u32;
                let src_ptr_local = temp_base;
                let len_local = temp_base + 1;
                let i_local = temp_base + 2;
                let acc_local = temp_base + 3;

                // Compile source list and store pointer
                self.compile_expr_with_locals(function, list, locals, locals_types)?;
                function.instruction(&Instruction::LocalSet(src_ptr_local));

                // Load source length
                function.instruction(&Instruction::LocalGet(src_ptr_local));
                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                function.instruction(&Instruction::LocalSet(len_local));

                // Initialize accumulator from init expression
                self.compile_expr_with_locals(function, init, locals, locals_types)?;
                function.instruction(&Instruction::LocalSet(acc_local));

                // Initialize i = 0
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::LocalSet(i_local));

                // Reduce loop
                function.instruction(&Instruction::Block(wasm_encoder::BlockType::Empty));
                function.instruction(&Instruction::Loop(wasm_encoder::BlockType::Empty));

                // Check: i >= len => break
                function.instruction(&Instruction::LocalGet(i_local));
                function.instruction(&Instruction::LocalGet(len_local));
                function.instruction(&Instruction::I32GeS);
                function.instruction(&Instruction::BrIf(1));

                // Compile lambda body with acc and elem bound
                if let Expr::Lambda { params, body, .. } = lambda.as_ref() {
                    let mut inner_locals = locals.clone();
                    // Bind first param to acc
                    if !params.is_empty() {
                        inner_locals.insert(params[0].name.clone(), acc_local);
                    }
                    // Bind second param to current element
                    if params.len() >= 2 {
                        // Load elem: src[4 + i*4]
                        function.instruction(&Instruction::LocalGet(src_ptr_local));
                        function.instruction(&Instruction::I32Const(4));
                        function.instruction(&Instruction::I32Add);
                        function.instruction(&Instruction::LocalGet(i_local));
                        function.instruction(&Instruction::I32Const(4));
                        function.instruction(&Instruction::I32Mul);
                        function.instruction(&Instruction::I32Add);
                        function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                            offset: 0,
                            align: 2,
                            memory_index: 0,
                        }));
                        // Need temp local for elem - use acc_local+1
                        let elem_local = acc_local + 1;
                        function.instruction(&Instruction::LocalSet(elem_local));
                        inner_locals.insert(params[1].name.clone(), elem_local);
                    }
                    self.compile_expr_with_locals(function, body, &inner_locals, locals_types)?;
                } else if let Expr::Ident(id) = lambda.as_ref() {
                    // Function variable - call_indirect with (acc, elem) -> result
                    if let Some(&local_idx) = locals.get(&id.name) {
                        // Load elem: src[4 + i*4]
                        let elem_local = acc_local + 1;
                        function.instruction(&Instruction::LocalGet(src_ptr_local));
                        function.instruction(&Instruction::I32Const(4));
                        function.instruction(&Instruction::I32Add);
                        function.instruction(&Instruction::LocalGet(i_local));
                        function.instruction(&Instruction::I32Const(4));
                        function.instruction(&Instruction::I32Mul);
                        function.instruction(&Instruction::I32Add);
                        function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                            offset: 0,
                            align: 2,
                            memory_index: 0,
                        }));
                        function.instruction(&Instruction::LocalSet(elem_local));
                        // Push args: acc, elem
                        function.instruction(&Instruction::LocalGet(acc_local));
                        function.instruction(&Instruction::LocalGet(elem_local));
                        // Load table_idx from closure cell offset 0
                        function.instruction(&Instruction::LocalGet(local_idx));
                        function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                            offset: 0,
                            align: 2,
                            memory_index: 0,
                        }));
                        // call_indirect with (i32, i32) -> i32 type
                        let type_idx =
                            self.get_or_create_type(&[ValType::I32, ValType::I32], &[ValType::I32]);
                        function.instruction(&Instruction::CallIndirect {
                            type_index: type_idx,
                            table_index: 0,
                        });
                    } else {
                        // Unknown - return acc unchanged
                        function.instruction(&Instruction::LocalGet(acc_local));
                    }
                } else {
                    // Not a lambda - just return acc
                    function.instruction(&Instruction::LocalGet(acc_local));
                }

                // Update accumulator with result
                function.instruction(&Instruction::LocalSet(acc_local));

                // i++
                function.instruction(&Instruction::LocalGet(i_local));
                function.instruction(&Instruction::I32Const(1));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalSet(i_local));

                function.instruction(&Instruction::Br(0));
                function.instruction(&Instruction::End); // end loop
                function.instruction(&Instruction::End); // end block

                // Return final accumulator value
                function.instruction(&Instruction::LocalGet(acc_local));
            }
            Expr::Ident(id) => {
                // Check for local variable first
                if let Some(&idx) = locals.get(&id.name) {
                    function.instruction(&Instruction::LocalGet(idx));
                } else if let Some(&(_, func_idx, _)) = self.functions.get(&id.name) {
                    function.instruction(&Instruction::Call(func_idx));
                } else {
                    function.instruction(&Instruction::I32Const(0));
                }
            }
            Expr::Call {
                func: callee, args, ..
            } => {
                for arg in args {
                    self.compile_expr_with_locals(function, arg, locals, locals_types)?;
                }
                if let Expr::Ident(id) = callee.as_ref() {
                    if let Some(&(_, func_idx, _)) = self.functions.get(&id.name) {
                        // Direct function call
                        function.instruction(&Instruction::Call(func_idx));
                    } else if let Some(&local_idx) = locals.get(&id.name) {
                        // Local variable holding a closure cell pointer
                        // Check if we know captures for this variable at compile time
                        if let Some(captures) = self.closure_info.get(&id.name).cloned() {
                            // We know the captures - generate unrolled unpacking
                            let temp_base = locals.len() as u32;
                            let cell_ptr_local = temp_base;

                            // Get closure cell pointer into temp
                            function.instruction(&Instruction::LocalGet(local_idx));
                            function.instruction(&Instruction::LocalSet(cell_ptr_local));

                            // Load each capture value from cell and push as args
                            for i in 0..captures.len() {
                                function.instruction(&Instruction::LocalGet(cell_ptr_local));
                                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                                    offset: ((2 + i) * 4) as u64,
                                    align: 2,
                                    memory_index: 0,
                                }));
                            }

                            // Load table_idx from cell offset 0
                            function.instruction(&Instruction::LocalGet(cell_ptr_local));
                            function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                                offset: 0,
                                align: 2,
                                memory_index: 0,
                            }));

                            // Type signature includes captures + regular args
                            let total_params = captures.len() + args.len();
                            let param_types: Vec<ValType> =
                                (0..total_params).map(|_| ValType::I32).collect();
                            let result_types = vec![ValType::I32];

                            let type_idx = self
                                .types
                                .iter()
                                .position(|(p, r)| p == &param_types && r == &result_types)
                                .map(|i| i as u32)
                                .unwrap_or_else(|| {
                                    let idx = self.types.len() as u32;
                                    self.types.push((param_types, result_types));
                                    idx
                                });

                            function.instruction(&Instruction::CallIndirect {
                                type_index: type_idx,
                                table_index: 0,
                            });
                        } else {
                            // No captures known - load table_idx from closure cell offset 0
                            function.instruction(&Instruction::LocalGet(local_idx));
                            function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                                offset: 0,
                                align: 2,
                                memory_index: 0,
                            }));

                            let param_types: Vec<ValType> =
                                args.iter().map(|_| ValType::I32).collect();
                            let result_types = vec![ValType::I32];

                            let type_idx = self
                                .types
                                .iter()
                                .position(|(p, r)| p == &param_types && r == &result_types)
                                .map(|i| i as u32)
                                .unwrap_or_else(|| {
                                    let idx = self.types.len() as u32;
                                    self.types.push((param_types, result_types));
                                    idx
                                });

                            function.instruction(&Instruction::CallIndirect {
                                type_index: type_idx,
                                table_index: 0,
                            });
                        }
                    } else {
                        // Unknown - drop args and return 0
                        for _ in args {
                            function.instruction(&Instruction::Drop);
                        }
                        function.instruction(&Instruction::I32Const(0));
                    }
                } else if let Expr::Field {
                    expr: receiver,
                    field,
                    ..
                } = callee.as_ref()
                {
                    // Qualified call: interface.function() or record.method()
                    if let Expr::Ident(interface_id) = receiver.as_ref() {
                        let interface_name = &interface_id.name;
                        let func_name = &field.name;

                        // Check if this is an imported interface call
                        if let Some((_, func_map)) = self.imported_interfaces.get(interface_name) {
                            if let Some(&func_idx) = func_map.get(func_name) {
                                function.instruction(&Instruction::Call(func_idx));
                            } else {
                                // Unknown function in interface - placeholder
                                for _ in args {
                                    function.instruction(&Instruction::Drop);
                                }
                                function.instruction(&Instruction::I32Const(0));
                            }
                        } else {
                            // Not an imported interface - try qualified function name
                            let qualified_name = format!("{}.{}", interface_name, func_name);
                            if let Some(&(_, func_idx, _)) = self.functions.get(&qualified_name) {
                                function.instruction(&Instruction::Call(func_idx));
                            } else if let Some(&(_, func_idx, _)) = self.functions.get(func_name) {
                                // Try direct function name
                                function.instruction(&Instruction::Call(func_idx));
                            } else {
                                // Fall back to indirect call
                                self.compile_expr_with_locals(
                                    function,
                                    callee,
                                    locals,
                                    locals_types,
                                )?;
                                let param_types: Vec<ValType> =
                                    args.iter().map(|_| ValType::I32).collect();
                                let result_types = vec![ValType::I32];
                                let type_idx = self
                                    .types
                                    .iter()
                                    .position(|(p, r)| p == &param_types && r == &result_types)
                                    .map(|i| i as u32)
                                    .unwrap_or_else(|| {
                                        let idx = self.types.len() as u32;
                                        self.types.push((param_types, result_types));
                                        idx
                                    });
                                function.instruction(&Instruction::CallIndirect {
                                    type_index: type_idx,
                                    table_index: 0,
                                });
                            }
                        }
                    } else {
                        // Complex receiver - fall back to indirect call
                        self.compile_expr_with_locals(function, callee, locals, locals_types)?;

                        let param_types: Vec<ValType> = args.iter().map(|_| ValType::I32).collect();
                        let result_types = vec![ValType::I32];

                        let type_idx = self
                            .types
                            .iter()
                            .position(|(p, r)| p == &param_types && r == &result_types)
                            .map(|i| i as u32)
                            .unwrap_or_else(|| {
                                let idx = self.types.len() as u32;
                                self.types.push((param_types, result_types));
                                idx
                            });

                        function.instruction(&Instruction::CallIndirect {
                            type_index: type_idx,
                            table_index: 0,
                        });
                    }
                } else {
                    // Indirect call - callee is an expression
                    self.compile_expr_with_locals(function, callee, locals, locals_types)?;

                    let param_types: Vec<ValType> = args.iter().map(|_| ValType::I32).collect();
                    let result_types = vec![ValType::I32];

                    let type_idx = self
                        .types
                        .iter()
                        .position(|(p, r)| p == &param_types && r == &result_types)
                        .map(|i| i as u32)
                        .unwrap_or_else(|| {
                            let idx = self.types.len() as u32;
                            self.types.push((param_types, result_types));
                            idx
                        });

                    function.instruction(&Instruction::CallIndirect {
                        type_index: type_idx,
                        table_index: 0,
                    });
                }
            }
            Expr::Field { expr, field, .. } => {
                // Compile the record expression (pushes pointer)
                self.compile_expr_with_locals(function, expr, locals, locals_types)?;

                // Calculate field offset - for now, assume 4 bytes per field
                let offset = if let Expr::RecordLiteral { fields, .. } = expr.as_ref() {
                    // Direct record literal: calculate from field order
                    fields
                        .iter()
                        .position(|(name, _)| name.name == field.name)
                        .map(|i| (i * 4) as u64)
                        .unwrap_or(0)
                } else if let Expr::Ident(id) = expr.as_ref() {
                    // Variable reference: look up type info from locals_types
                    if let Some(type_info) = locals_types.get(&id.name) {
                        type_info.get_offset(&field.name).unwrap_or(0) as u64
                    } else {
                        0
                    }
                } else {
                    // Other expressions: can't determine offset
                    0
                };

                // Load i32 at offset
                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Expr::Binary { lhs, op, rhs, .. } => {
                use kettu_parser::BinOp;
                match op {
                    // Short-circuit &&: if lhs is false, result is false; else result is rhs
                    BinOp::And => {
                        self.compile_expr_with_locals(function, lhs, locals, locals_types)?;
                        function.instruction(&Instruction::If(wasm_encoder::BlockType::Result(
                            wasm_encoder::ValType::I32,
                        )));
                        self.compile_expr_with_locals(function, rhs, locals, locals_types)?;
                        function.instruction(&Instruction::Else);
                        function.instruction(&Instruction::I32Const(0));
                        function.instruction(&Instruction::End);
                    }
                    // Short-circuit ||: if lhs is true, result is true; else result is rhs
                    BinOp::Or => {
                        self.compile_expr_with_locals(function, lhs, locals, locals_types)?;
                        function.instruction(&Instruction::If(wasm_encoder::BlockType::Result(
                            wasm_encoder::ValType::I32,
                        )));
                        function.instruction(&Instruction::I32Const(1));
                        function.instruction(&Instruction::Else);
                        self.compile_expr_with_locals(function, rhs, locals, locals_types)?;
                        function.instruction(&Instruction::End);
                    }
                    // Non-short-circuit operators: evaluate both sides
                    _ => {
                        self.compile_expr_with_locals(function, lhs, locals, locals_types)?;
                        self.compile_expr_with_locals(function, rhs, locals, locals_types)?;
                        match op {
                            BinOp::Add => {
                                // Check if this is string concatenation
                                if Self::is_string_expr(lhs) {
                                    let concat_idx = self.ensure_str_concat_func();
                                    function.instruction(&Instruction::Call(concat_idx))
                                } else {
                                    function.instruction(&Instruction::I32Add)
                                }
                            }
                            BinOp::Sub => function.instruction(&Instruction::I32Sub),
                            BinOp::Mul => function.instruction(&Instruction::I32Mul),
                            BinOp::Div => function.instruction(&Instruction::I32DivS),
                            BinOp::Eq => function.instruction(&Instruction::I32Eq),
                            BinOp::Ne => function.instruction(&Instruction::I32Ne),
                            BinOp::Lt => function.instruction(&Instruction::I32LtS),
                            BinOp::Le => function.instruction(&Instruction::I32LeS),
                            BinOp::Gt => function.instruction(&Instruction::I32GtS),
                            BinOp::Ge => function.instruction(&Instruction::I32GeS),
                            BinOp::And | BinOp::Or => unreachable!(),
                        };
                    }
                }
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                self.compile_expr_with_locals(function, cond, locals, locals_types)?;
                // If expression returns i32 (bool is i32 in WASM)
                function.instruction(&Instruction::If(wasm_encoder::BlockType::Result(
                    wasm_encoder::ValType::I32,
                )));

                // Helper to compile branch with value left on stack
                fn compile_branch_with_locals(
                    compiler: &mut ModuleCompiler,
                    function: &mut Function,
                    stmts: &[Statement],
                    locals: &HashMap<String, u32>,
                    locals_types: &mut HashMap<String, RecordTypeInfo>,
                ) -> Result<(), CompileError> {
                    if stmts.is_empty() {
                        function.instruction(&Instruction::I32Const(0));
                        return Ok(());
                    }
                    for stmt in &stmts[..stmts.len() - 1] {
                        compiler.compile_statement_with_locals(
                            function,
                            stmt,
                            locals,
                            locals_types,
                        )?;
                    }
                    // Last statement: leave value on stack
                    match &stmts[stmts.len() - 1] {
                        Statement::Expr(expr) => {
                            compiler.compile_expr_with_locals(
                                function,
                                expr,
                                locals,
                                locals_types,
                            )?;
                        }
                        Statement::Return(Some(expr)) => {
                            compiler.compile_expr_with_locals(
                                function,
                                expr,
                                locals,
                                locals_types,
                            )?;
                            function.instruction(&Instruction::Return);
                        }
                        Statement::Return(None) => {
                            function.instruction(&Instruction::I32Const(0));
                            function.instruction(&Instruction::Return);
                        }
                        Statement::Let { value, .. } => {
                            compiler.compile_expr_with_locals(
                                function,
                                value,
                                locals,
                                locals_types,
                            )?;
                        }
                        Statement::Assign { value, .. }
                        | Statement::CompoundAssign { value, .. } => {
                            compiler.compile_expr_with_locals(
                                function,
                                value,
                                locals,
                                locals_types,
                            )?;
                        }
                        Statement::Break { .. } => {
                            // Break as last statement in if branch
                            if let Some(depth) = compiler.loop_break_depth {
                                function.instruction(&Instruction::Br(depth));
                            }
                            function.instruction(&Instruction::I32Const(0));
                        }
                        Statement::Continue { .. } => {
                            // Continue as last statement in if branch
                            if let Some(depth) = compiler.loop_continue_depth {
                                function.instruction(&Instruction::Br(depth));
                            }
                            function.instruction(&Instruction::I32Const(0));
                        }
                        Statement::SharedLet { .. } => {
                            function.instruction(&Instruction::I32Const(0));
                        }
                    }
                    Ok(())
                }

                // Bump loop depths for if-block nesting (+1 for the if block)
                if let Some(d) = self.loop_break_depth.as_mut() { *d += 1; }
                if let Some(d) = self.loop_continue_depth.as_mut() { *d += 1; }

                // Need mutable clone since helper needs mut ref
                let mut types_clone = locals_types.clone();
                compile_branch_with_locals(self, function, then_branch, locals, &mut types_clone)?;

                if let Some(else_stmts) = else_branch {
                    function.instruction(&Instruction::Else);
                    let mut types_clone2 = locals_types.clone();
                    compile_branch_with_locals(
                        self,
                        function,
                        else_stmts,
                        locals,
                        &mut types_clone2,
                    )?;
                } else {
                    function.instruction(&Instruction::Else);
                    function.instruction(&Instruction::I32Const(0));
                }
                function.instruction(&Instruction::End);

                // Restore loop depths after if-block
                if let Some(d) = self.loop_break_depth.as_mut() { *d -= 1; }
                if let Some(d) = self.loop_continue_depth.as_mut() { *d -= 1; }
            }
            Expr::Assert(cond, _) => {
                // Compile condition
                self.compile_expr_with_locals(function, cond, locals, locals_types)?;
                // If false, trap with unreachable
                function.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
                // Condition was true - do nothing
                function.instruction(&Instruction::Else);
                // Condition was false - trap
                function.instruction(&Instruction::Unreachable);
                function.instruction(&Instruction::End);
                // Leave true on stack (assert passed)
                function.instruction(&Instruction::I32Const(1));
            }
            Expr::Not(expr, _) => {
                // Compile the expression
                self.compile_expr_with_locals(function, expr, locals, locals_types)?;
                // Negate: 0 -> 1, non-zero -> 0
                function.instruction(&Instruction::I32Eqz);
            }
            Expr::Neg(expr, _) => {
                // Unary negation: 0 - expr
                function.instruction(&Instruction::I32Const(0));
                self.compile_expr_with_locals(function, expr, locals, locals_types)?;
                function.instruction(&Instruction::I32Sub);
            }
            Expr::StrLen(expr, _) => {
                // Compile string expression (pushes pointer)
                self.compile_expr_with_locals(function, expr, locals, locals_types)?;
                // Read length from ptr - 4
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Sub);
                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Expr::StrEq(a, b, _) => {
                // Compare two strings by content using builtin str_eq function
                let str_eq_idx = self.ensure_str_eq_func();
                self.compile_expr_with_locals(function, a, locals, locals_types)?;
                self.compile_expr_with_locals(function, b, locals, locals_types)?;
                function.instruction(&Instruction::Call(str_eq_idx));
            }
            Expr::ListLen(expr, _) => {
                // Compile list expression (pushes pointer)
                self.compile_expr_with_locals(function, expr, locals, locals_types)?;
                // Read length from offset 0 (list layout: [len:i32][elem0][elem1]...)
                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Expr::ListSet(arr_expr, idx_expr, val_expr, _) => {
                // list-set(arr, idx, val): store val at arr[idx], return arr
                // Memory layout: [len:i32][elem0][elem1]...
                // Address = arr + 4 + idx * 4

                // Use temp local to store arr pointer
                let temp_local = locals.len() as u32;

                // Compile arr and store in temp
                self.compile_expr_with_locals(function, arr_expr, locals, locals_types)?;
                function.instruction(&Instruction::LocalSet(temp_local));

                // Compute address: arr + 4 + idx * 4
                function.instruction(&Instruction::LocalGet(temp_local));
                function.instruction(&Instruction::I32Const(4)); // skip length
                function.instruction(&Instruction::I32Add);
                self.compile_expr_with_locals(function, idx_expr, locals, locals_types)?;
                function.instruction(&Instruction::I32Const(4)); // element size
                function.instruction(&Instruction::I32Mul);
                function.instruction(&Instruction::I32Add);

                // Compile value
                self.compile_expr_with_locals(function, val_expr, locals, locals_types)?;

                // Store value at computed address
                function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));

                // Return arr pointer for chaining
                function.instruction(&Instruction::LocalGet(temp_local));
            }
            Expr::RecordLiteral { fields, .. } => {
                // Allocate space for record (4 bytes per field for i32 values)
                let size = (fields.len() * 4) as i32;
                let alloc_idx = self.alloc_func_idx.unwrap();
                // Use temp local at end of locals array
                let temp_local = locals.len() as u32;

                // Call alloc(size) and store result in temp local
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::I32Const(size));
                function.instruction(&Instruction::Call(alloc_idx));
                function.instruction(&Instruction::LocalSet(temp_local));

                // Store each field value at offset
                for (i, (_, expr)) in fields.iter().enumerate() {
                    // Push pointer from temp local
                    function.instruction(&Instruction::LocalGet(temp_local));
                    // Compile field value
                    self.compile_expr_with_locals(function, expr, locals, locals_types)?;
                    // Store at offset: expects [ptr, value] on stack
                    function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                        offset: (i * 4) as u64,
                        align: 2,
                        memory_index: 0,
                    }));
                }
                // Push pointer back on stack as return value
                function.instruction(&Instruction::LocalGet(temp_local));
            }
            Expr::VariantLiteral {
                case_name, payload, ..
            } => {
                // Variant layout: [discriminant: i32] [payload: i32 (optional)]
                // For now, discriminant is hash of case name (simple approach)
                let discriminant = case_name
                    .name
                    .bytes()
                    .fold(0u32, |acc, b| acc.wrapping_add(b as u32));
                let temp_local = locals.len() as u32;

                if payload.is_some() {
                    // Allocate 8 bytes: 4 for discriminant, 4 for payload
                    let alloc_idx = self.alloc_func_idx.unwrap();
                    function.instruction(&Instruction::I32Const(0)); // ptr (unused)
                    function.instruction(&Instruction::I32Const(0)); // old_size (unused)
                    function.instruction(&Instruction::I32Const(0)); // align (unused)
                    function.instruction(&Instruction::I32Const(8));
                    function.instruction(&Instruction::Call(alloc_idx));
                    function.instruction(&Instruction::LocalSet(temp_local));

                    // Store discriminant at offset 0
                    function.instruction(&Instruction::LocalGet(temp_local));
                    function.instruction(&Instruction::I32Const(discriminant as i32));
                    function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                        offset: 0,
                        align: 2,
                        memory_index: 0,
                    }));

                    // Store payload at offset 4
                    function.instruction(&Instruction::LocalGet(temp_local));
                    self.compile_expr_with_locals(
                        function,
                        payload.as_ref().unwrap(),
                        locals,
                        locals_types,
                    )?;
                    function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                        offset: 4,
                        align: 2,
                        memory_index: 0,
                    }));

                    // Push pointer back
                    function.instruction(&Instruction::LocalGet(temp_local));
                } else {
                    // No payload - just push discriminant as value
                    function.instruction(&Instruction::I32Const(discriminant as i32));
                }
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                // Match expression: compile as if-else chain on discriminant
                // Compile scrutinee (could be a discriminant value or a pointer)
                self.compile_expr_with_locals(function, scrutinee, locals, locals_types)?;

                // Store scrutinee in temp local
                let scrutinee_local = locals.len() as u32;
                function.instruction(&Instruction::LocalSet(scrutinee_local));

                // Check if any arm has a binding - if so, we're matching a pointer variant
                let has_payload_binding = arms.iter().any(|arm| {
                    matches!(
                        &arm.pattern,
                        Pattern::Variant {
                            binding: Some(_),
                            ..
                        }
                    )
                });

                if arms.is_empty() {
                    // No arms - push default value
                    function.instruction(&Instruction::I32Const(0));
                } else {
                    // Build nested if-else chain
                    for (i, arm) in arms.iter().enumerate() {
                        let is_last = i == arms.len() - 1;

                        if !is_last {
                            // Match on discriminant
                            if let Pattern::Variant { case_name, .. } = &arm.pattern {
                                // Compute expected discriminant
                                let expected = case_name
                                    .name
                                    .bytes()
                                    .fold(0u32, |acc, b| acc.wrapping_add(b as u32));

                                if has_payload_binding {
                                    // Scrutinee is a pointer - load discriminant from memory
                                    function.instruction(&Instruction::LocalGet(scrutinee_local));
                                    function.instruction(&Instruction::I32Load(
                                        wasm_encoder::MemArg {
                                            offset: 0,
                                            align: 2,
                                            memory_index: 0,
                                        },
                                    ));
                                } else {
                                    // Scrutinee is a direct discriminant value
                                    function.instruction(&Instruction::LocalGet(scrutinee_local));
                                }
                                function.instruction(&Instruction::I32Const(expected as i32));
                                function.instruction(&Instruction::I32Eq);

                                // If matching
                                function.instruction(&Instruction::If(
                                    wasm_encoder::BlockType::Result(wasm_encoder::ValType::I32),
                                ));
                            } else if let Pattern::Wildcard(_) = &arm.pattern {
                                // Wildcard always matches - emit body directly later
                            }
                        }

                        // Handle payload binding if present
                        if let Pattern::Variant {
                            binding: Some(binding_id),
                            ..
                        } = &arm.pattern
                        {
                            // Load payload from memory[scrutinee + 4] into a new local
                            // Create extended locals map with the binding
                            let binding_local = scrutinee_local + 1; // Use next available local
                            function.instruction(&Instruction::LocalGet(scrutinee_local));
                            function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                                offset: 4,
                                align: 2,
                                memory_index: 0,
                            }));
                            function.instruction(&Instruction::LocalSet(binding_local));

                            // Create new locals map with binding
                            let mut arm_locals = locals.clone();
                            arm_locals.insert(binding_id.name.clone(), binding_local);

                            // Compile arm body with binding available
                            if let Some(Statement::Expr(body_expr)) = arm.body.first() {
                                self.compile_expr_with_locals(
                                    function,
                                    body_expr,
                                    &arm_locals,
                                    locals_types,
                                )?;
                            } else {
                                function.instruction(&Instruction::I32Const(0));
                            }
                        } else {
                            // No binding - compile body with original locals
                            if let Some(Statement::Expr(body_expr)) = arm.body.first() {
                                self.compile_expr_with_locals(
                                    function,
                                    body_expr,
                                    locals,
                                    locals_types,
                                )?;
                            } else {
                                function.instruction(&Instruction::I32Const(0));
                            }
                        }

                        if !is_last {
                            function.instruction(&Instruction::Else);
                        }
                    }

                    // Close all the if-else chains (N-1 End instructions for N arms)
                    for _ in 0..(arms.len() - 1) {
                        function.instruction(&Instruction::End);
                    }
                }
            }
            Expr::While {
                condition, body, ..
            } => {
                // WASM while loop pattern:
                // (block $break
                //   (loop $continue
                //     br_if $break (i32.eqz condition)
                //     body
                //     br $continue
                //   )
                // )

                // Outer block for break target
                function.instruction(&Instruction::Block(wasm_encoder::BlockType::Empty));
                // Inner loop for continue
                function.instruction(&Instruction::Loop(wasm_encoder::BlockType::Empty));

                // Save and set loop depths: at body level, break=Br(1), continue=Br(0)
                let prev_break = self.loop_break_depth;
                let prev_continue = self.loop_continue_depth;
                self.loop_break_depth = Some(1);
                self.loop_continue_depth = Some(0);

                // Evaluate condition
                self.compile_expr_with_locals(function, condition, locals, locals_types)?;
                // If condition is false (eqz), break out
                function.instruction(&Instruction::I32Eqz);
                function.instruction(&Instruction::BrIf(1)); // break to outer block
                for stmt in body {
                    match stmt {
                        Statement::Expr(e) => {
                            self.compile_expr_with_locals(function, e, locals, locals_types)?;
                            // Drop result of expression
                            function.instruction(&Instruction::Drop);
                        }
                        Statement::Assign { name, value } => {
                            // Compile value and store to local
                            self.compile_expr_with_locals(function, value, locals, locals_types)?;
                            if let Some(&idx) = locals.get(&name.name) {
                                function.instruction(&Instruction::LocalSet(idx));
                            }
                        }
                        Statement::CompoundAssign { name, op, value } => {
                            if let Some(&idx) = locals.get(&name.name) {
                                function.instruction(&Instruction::LocalGet(idx));
                            }
                            self.compile_expr_with_locals(function, value, locals, locals_types)?;
                            match op {
                                BinOp::Add => { function.instruction(&Instruction::I32Add); }
                                BinOp::Sub => { function.instruction(&Instruction::I32Sub); }
                                _ => { function.instruction(&Instruction::I32Add); }
                            }
                            if let Some(&idx) = locals.get(&name.name) {
                                function.instruction(&Instruction::LocalSet(idx));
                            }
                        }
                        Statement::Let { name, value } => {
                            // Compile value and store to local
                            self.compile_expr_with_locals(function, value, locals, locals_types)?;
                            if let Some(&idx) = locals.get(&name.name) {
                                function.instruction(&Instruction::LocalSet(idx));
                            }
                        }
                        Statement::Return(opt_e) => {
                            if let Some(e) = opt_e {
                                self.compile_expr_with_locals(function, e, locals, locals_types)?;
                            }
                            function.instruction(&Instruction::Return);
                        }
                        Statement::Break { condition } => {
                            // break = branch to outer block (exit loop)
                            if let Some(cond) = condition {
                                // break if <cond>: only break if condition is true
                                self.compile_expr_with_locals(
                                    function,
                                    cond,
                                    locals,
                                    locals_types,
                                )?;
                                function.instruction(&Instruction::BrIf(1));
                            } else {
                                function.instruction(&Instruction::Br(1));
                            }
                        }
                        Statement::Continue { condition } => {
                            // continue = branch to loop start
                            if let Some(cond) = condition {
                                // continue if <cond>: only continue if condition is true
                                self.compile_expr_with_locals(
                                    function,
                                    cond,
                                    locals,
                                    locals_types,
                                )?;
                                function.instruction(&Instruction::BrIf(0));
                            } else {
                                function.instruction(&Instruction::Br(0));
                            }
                        }
                        Statement::SharedLet { .. } => {}
                    }
                }

                // Branch back to loop start
                function.instruction(&Instruction::Br(0));
                function.instruction(&Instruction::End); // end loop
                function.instruction(&Instruction::End); // end block

                // Restore loop depths
                self.loop_break_depth = prev_break;
                self.loop_continue_depth = prev_continue;

                // While returns unit (push 0)
                function.instruction(&Instruction::I32Const(0));
            }
            Expr::Range { .. } => {
                // Range expressions are only valid inside for loops
                function.instruction(&Instruction::I32Const(0));
            }
            Expr::For {
                variable,
                range,
                body,
                ..
            } => {
                // For loop: for var in start to end { body }
                // Inclusive range: iterates from start to end (inclusive)
                //
                // WASM pattern:
                // (block $break
                //   (loop $continue
                //     local.get $i
                //     local.get $end
                //     i32.gt_s           ;; if i > end, break
                //     br_if $break
                //     body
                //     local.get $i
                //     i32.const 1
                //     i32.add
                //     local.set $i
                //     br $continue
                //   )
                // )

                // Extract start, end, step, and descending from range
                if let Expr::Range {
                    start,
                    end,
                    step,
                    descending,
                    ..
                } = range.as_ref()
                {
                    // Get loop variable index
                    let var_idx = *locals.get(&variable.name).expect("loop variable not found");

                    // Compile and store start value to loop variable
                    self.compile_expr_with_locals(function, start, locals, locals_types)?;
                    function.instruction(&Instruction::LocalSet(var_idx));

                    // We need a temporary for 'end' value
                    // For now, we'll recompute end each iteration (simple but works)

                    // Structure: block $break > loop $loop > block $body
                    // break -> br 2 (exits outer block)
                    // continue -> br 0 (exits body block, falls through to increment)
                    // loop back -> br 1 (branches to loop)

                    // Outer block for break target (depth 2 from body)
                    function.instruction(&Instruction::Block(wasm_encoder::BlockType::Empty));
                    // Loop (depth 1 from body)
                    function.instruction(&Instruction::Loop(wasm_encoder::BlockType::Empty));

                    // Check termination condition based on direction
                    function.instruction(&Instruction::LocalGet(var_idx));
                    self.compile_expr_with_locals(function, end, locals, locals_types)?;
                    if *descending {
                        // For downto: break if i < end
                        function.instruction(&Instruction::I32LtS);
                    } else {
                        // For to: break if i > end
                        function.instruction(&Instruction::I32GtS);
                    }
                    function.instruction(&Instruction::BrIf(1)); // break to outer block

                    // Inner block for body (continue target) - depth 0 from body
                    function.instruction(&Instruction::Block(wasm_encoder::BlockType::Empty));

                    // Save and set loop depths for for-range:
                    // From body: break=Br(2) to outer block, continue=Br(0) to body block end
                    let prev_break = self.loop_break_depth;
                    let prev_continue = self.loop_continue_depth;
                    self.loop_break_depth = Some(2);
                    self.loop_continue_depth = Some(0);

                    // Compile body statements
                    for stmt in body {
                        match stmt {
                            Statement::Expr(e) => {
                                self.compile_expr_with_locals(function, e, locals, locals_types)?;
                                function.instruction(&Instruction::Drop);
                            }
                            Statement::Assign { name, value } => {
                                self.compile_expr_with_locals(
                                    function,
                                    value,
                                    locals,
                                    locals_types,
                                )?;
                                if let Some(&idx) = locals.get(&name.name) {
                                    function.instruction(&Instruction::LocalSet(idx));
                                }
                            }
                            Statement::CompoundAssign { name, op, value } => {
                                if let Some(&idx) = locals.get(&name.name) {
                                    function.instruction(&Instruction::LocalGet(idx));
                                }
                                self.compile_expr_with_locals(function, value, locals, locals_types)?;
                                match op {
                                    BinOp::Add => { function.instruction(&Instruction::I32Add); }
                                    BinOp::Sub => { function.instruction(&Instruction::I32Sub); }
                                    _ => { function.instruction(&Instruction::I32Add); }
                                }
                                if let Some(&idx) = locals.get(&name.name) {
                                    function.instruction(&Instruction::LocalSet(idx));
                                }
                            }
                            Statement::Let { name, value } => {
                                self.compile_expr_with_locals(
                                    function,
                                    value,
                                    locals,
                                    locals_types,
                                )?;
                                if let Some(&idx) = locals.get(&name.name) {
                                    function.instruction(&Instruction::LocalSet(idx));
                                }
                            }
                            Statement::Return(opt_e) => {
                                if let Some(e) = opt_e {
                                    self.compile_expr_with_locals(
                                        function,
                                        e,
                                        locals,
                                        locals_types,
                                    )?;
                                }
                                function.instruction(&Instruction::Return);
                            }
                            Statement::Break { condition } => {
                                // break -> br 2 (exit outer block, exit loop)
                                if let Some(cond) = condition {
                                    self.compile_expr_with_locals(
                                        function,
                                        cond,
                                        locals,
                                        locals_types,
                                    )?;
                                    function.instruction(&Instruction::BrIf(2));
                                } else {
                                    function.instruction(&Instruction::Br(2));
                                }
                            }
                            Statement::Continue { condition } => {
                                // continue -> br 0 (exit body block, fall through to increment)
                                if let Some(cond) = condition {
                                    self.compile_expr_with_locals(
                                        function,
                                        cond,
                                        locals,
                                        locals_types,
                                    )?;
                                    function.instruction(&Instruction::BrIf(0));
                                } else {
                                    function.instruction(&Instruction::Br(0));
                                }
                            }
                            Statement::SharedLet { .. } => {}
                        }
                    }

                    function.instruction(&Instruction::End); // end body block

                    // Update loop variable based on step and direction
                    function.instruction(&Instruction::LocalGet(var_idx));
                    if let Some(step_expr) = step {
                        self.compile_expr_with_locals(function, step_expr, locals, locals_types)?;
                    } else {
                        function.instruction(&Instruction::I32Const(1));
                    }
                    if *descending {
                        function.instruction(&Instruction::I32Sub);
                    } else {
                        function.instruction(&Instruction::I32Add);
                    }
                    function.instruction(&Instruction::LocalSet(var_idx));

                    // Branch back to loop start
                    function.instruction(&Instruction::Br(0));
                    function.instruction(&Instruction::End); // end loop
                    function.instruction(&Instruction::End); // end block

                    // Restore loop depths
                    self.loop_break_depth = prev_break;
                    self.loop_continue_depth = prev_continue;
                }

                // For returns unit (push 0)
                function.instruction(&Instruction::I32Const(0));
            }
            Expr::ListLiteral { elements, .. } => {
                // Memory layout: [length: i32][elem0][elem1]...
                // Each element is 4 bytes (i32)
                let num_elements = elements.len() as i32;
                let size = 4 + num_elements * 4; // length + elements

                // Use temp local at end of locals array
                let temp_local = locals.len() as u32;
                let alloc_idx = self.alloc_func_idx.unwrap();

                // Call alloc(size) and store result in temp local
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::I32Const(size));
                function.instruction(&Instruction::Call(alloc_idx));
                function.instruction(&Instruction::LocalSet(temp_local));

                // Store length at base pointer
                function.instruction(&Instruction::LocalGet(temp_local));
                function.instruction(&Instruction::I32Const(num_elements));
                function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));

                // Store each element at offset 4 + i*4
                for (i, elem) in elements.iter().enumerate() {
                    function.instruction(&Instruction::LocalGet(temp_local)); // base pointer
                    self.compile_expr_with_locals(function, elem, locals, locals_types)?;
                    function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                        offset: (4 + i * 4) as u64,
                        align: 2,
                        memory_index: 0,
                    }));
                }

                // Leave base pointer on stack as return value
                function.instruction(&Instruction::LocalGet(temp_local));
            }
            Expr::Index { expr, index, .. } => {
                // Load element from list at index
                // list_ptr + 4 + index * 4

                // Compile base expression (list pointer)
                self.compile_expr_with_locals(function, expr, locals, locals_types)?;
                function.instruction(&Instruction::I32Const(4)); // skip length
                function.instruction(&Instruction::I32Add);

                // Compile index and multiply by element size
                self.compile_expr_with_locals(function, index, locals, locals_types)?;
                function.instruction(&Instruction::I32Const(4)); // element size
                function.instruction(&Instruction::I32Mul);
                function.instruction(&Instruction::I32Add);

                // Load element value
                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Expr::Slice {
                expr, start, end, ..
            } => {
                // Slice: arr[start..end] creates new list with (end - start) elements
                // Uses temp locals: src_ptr, start_idx, end_idx, len, dest_ptr, i
                let temp_base = locals.len() as u32;
                let src_ptr_local = temp_base;
                let start_local = temp_base + 1;
                let end_local = temp_base + 2;
                let len_local = temp_base + 3;
                let dest_ptr_local = temp_base + 4;
                let i_local = temp_base + 5;

                // Compile source list, start, end
                self.compile_expr_with_locals(function, expr, locals, locals_types)?;
                function.instruction(&Instruction::LocalSet(src_ptr_local));
                self.compile_expr_with_locals(function, start, locals, locals_types)?;
                function.instruction(&Instruction::LocalSet(start_local));
                self.compile_expr_with_locals(function, end, locals, locals_types)?;
                function.instruction(&Instruction::LocalSet(end_local));

                // Calculate length: end - start
                function.instruction(&Instruction::LocalGet(end_local));
                function.instruction(&Instruction::LocalGet(start_local));
                function.instruction(&Instruction::I32Sub);
                function.instruction(&Instruction::LocalSet(len_local));

                // Allocate new list: 4 (length) + len * 4 (elements)
                // Push CABI args: (0, 0, 0, size)
                function.instruction(&Instruction::I32Const(0)); // ptr (unused)
                function.instruction(&Instruction::I32Const(0)); // old_size (unused)
                function.instruction(&Instruction::I32Const(0)); // align (unused)
                function.instruction(&Instruction::LocalGet(len_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Mul);
                function.instruction(&Instruction::I32Const(4)); // for length field
                function.instruction(&Instruction::I32Add);
                let alloc_idx = self.alloc_func_idx.unwrap();
                function.instruction(&Instruction::Call(alloc_idx));
                function.instruction(&Instruction::LocalSet(dest_ptr_local));

                // Store length at dest[0]
                function.instruction(&Instruction::LocalGet(dest_ptr_local));
                function.instruction(&Instruction::LocalGet(len_local));
                function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));

                // Initialize i = 0
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::LocalSet(i_local));

                // Copy loop
                function.instruction(&Instruction::Block(wasm_encoder::BlockType::Empty));
                function.instruction(&Instruction::Loop(wasm_encoder::BlockType::Empty));

                // Check: i >= len => break
                function.instruction(&Instruction::LocalGet(i_local));
                function.instruction(&Instruction::LocalGet(len_local));
                function.instruction(&Instruction::I32GeS);
                function.instruction(&Instruction::BrIf(1));

                // dest[4 + i*4] = src[4 + (start + i)*4]
                // Compute dest address
                function.instruction(&Instruction::LocalGet(dest_ptr_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalGet(i_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Mul);
                function.instruction(&Instruction::I32Add);

                // Compute src address
                function.instruction(&Instruction::LocalGet(src_ptr_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalGet(start_local));
                function.instruction(&Instruction::LocalGet(i_local));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Mul);
                function.instruction(&Instruction::I32Add);

                // Load src value
                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));

                // Store to dest
                function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));

                // i++
                function.instruction(&Instruction::LocalGet(i_local));
                function.instruction(&Instruction::I32Const(1));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalSet(i_local));

                // br 0 (continue loop)
                function.instruction(&Instruction::Br(0));
                function.instruction(&Instruction::End); // end loop
                function.instruction(&Instruction::End); // end block

                // Return dest pointer
                function.instruction(&Instruction::LocalGet(dest_ptr_local));
            }
            Expr::ListPush(arr_expr, val_expr, _) => {
                // list-push(arr, val): return new list with val appended
                // Memory layout: [len:i32][elem0][elem1]...
                // New layout: [len+1:i32][elem0][elem1]...[val]
                let temp_base = locals.len() as u32;
                let src_ptr_local = temp_base;
                let len_local = temp_base + 1;
                let dest_ptr_local = temp_base + 2;
                let i_local = temp_base + 3;
                let val_local = temp_base + 4;

                // Compile source list and store pointer
                self.compile_expr_with_locals(function, arr_expr, locals, locals_types)?;
                function.instruction(&Instruction::LocalSet(src_ptr_local));

                // Compile val and store
                self.compile_expr_with_locals(function, val_expr, locals, locals_types)?;
                function.instruction(&Instruction::LocalSet(val_local));

                // Load source length
                function.instruction(&Instruction::LocalGet(src_ptr_local));
                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                function.instruction(&Instruction::LocalSet(len_local));

                // Allocate new list: 4 (length) + (len + 1) * 4
                // Push CABI args: (0, 0, 0, size)
                function.instruction(&Instruction::I32Const(0)); // ptr (unused)
                function.instruction(&Instruction::I32Const(0)); // old_size (unused)
                function.instruction(&Instruction::I32Const(0)); // align (unused)
                function.instruction(&Instruction::LocalGet(len_local));
                function.instruction(&Instruction::I32Const(1));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Mul);
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Add);
                let alloc_idx = self.alloc_func_idx.unwrap();
                function.instruction(&Instruction::Call(alloc_idx));
                function.instruction(&Instruction::LocalSet(dest_ptr_local));

                // Store new length (len + 1) at dest[0]
                function.instruction(&Instruction::LocalGet(dest_ptr_local));
                function.instruction(&Instruction::LocalGet(len_local));
                function.instruction(&Instruction::I32Const(1));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));

                // Initialize i = 0
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::LocalSet(i_local));

                // Copy loop
                function.instruction(&Instruction::Block(wasm_encoder::BlockType::Empty));
                function.instruction(&Instruction::Loop(wasm_encoder::BlockType::Empty));

                // Check: i >= len => break
                function.instruction(&Instruction::LocalGet(i_local));
                function.instruction(&Instruction::LocalGet(len_local));
                function.instruction(&Instruction::I32GeS);
                function.instruction(&Instruction::BrIf(1));

                // dest[4 + i*4] = src[4 + i*4]
                function.instruction(&Instruction::LocalGet(dest_ptr_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalGet(i_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Mul);
                function.instruction(&Instruction::I32Add);

                function.instruction(&Instruction::LocalGet(src_ptr_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalGet(i_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Mul);
                function.instruction(&Instruction::I32Add);

                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));

                function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));

                // i++
                function.instruction(&Instruction::LocalGet(i_local));
                function.instruction(&Instruction::I32Const(1));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalSet(i_local));

                function.instruction(&Instruction::Br(0));
                function.instruction(&Instruction::End); // end loop
                function.instruction(&Instruction::End); // end block

                // Store val at dest[4 + len*4]
                function.instruction(&Instruction::LocalGet(dest_ptr_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalGet(len_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Mul);
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalGet(val_local));
                function.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));

                // Return dest pointer
                function.instruction(&Instruction::LocalGet(dest_ptr_local));
            }
            Expr::ForEach {
                variable,
                collection,
                body,
                ..
            } => {
                // For-each loop: for item in collection { body }
                // We iterate using an index from 0 to length-1
                //
                // Layout: [length: i32][elem0][elem1]...
                // Locals needed: list_ptr, idx, elem (the variable)
                //
                // WASM pattern:
                // (block $break
                //   (loop $continue
                //     idx < length ? continue : break
                //     elem = list[idx]
                //     body
                //     idx++
                //     br $continue
                //   )
                // )

                // Use temp locals
                let list_ptr_local = locals.len() as u32;
                let idx_local = list_ptr_local + 1;
                let elem_local = *locals
                    .get(&variable.name)
                    .expect("for-each variable not found");

                // Compile collection and store pointer
                self.compile_expr_with_locals(function, collection, locals, locals_types)?;
                function.instruction(&Instruction::LocalSet(list_ptr_local));

                // Initialize index to 0
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::LocalSet(idx_local));

                // Outer block for break target
                function.instruction(&Instruction::Block(wasm_encoder::BlockType::Empty));
                // Inner loop for iteration
                function.instruction(&Instruction::Loop(wasm_encoder::BlockType::Empty));

                // Save and set loop depths: at body level, break=Br(1), continue=Br(0)
                let prev_break = self.loop_break_depth;
                let prev_continue = self.loop_continue_depth;
                self.loop_break_depth = Some(1);
                self.loop_continue_depth = Some(0);

                // Check termination: idx >= length => break
                function.instruction(&Instruction::LocalGet(idx_local));
                // Load length from list_ptr[0]
                function.instruction(&Instruction::LocalGet(list_ptr_local));
                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                function.instruction(&Instruction::I32GeS);
                function.instruction(&Instruction::BrIf(1)); // break to outer block

                // Load element at idx into elem_local
                // elem = list_ptr + 4 + idx * 4
                function.instruction(&Instruction::LocalGet(list_ptr_local));
                function.instruction(&Instruction::I32Const(4)); // skip length
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalGet(idx_local));
                function.instruction(&Instruction::I32Const(4)); // element size
                function.instruction(&Instruction::I32Mul);
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                function.instruction(&Instruction::LocalSet(elem_local));

                // Compile body statements
                for stmt in body {
                    match stmt {
                        Statement::Expr(e) => {
                            self.compile_expr_with_locals(function, e, locals, locals_types)?;
                            function.instruction(&Instruction::Drop);
                        }
                        Statement::Assign { name, value } => {
                            self.compile_expr_with_locals(function, value, locals, locals_types)?;
                            if let Some(&idx) = locals.get(&name.name) {
                                function.instruction(&Instruction::LocalSet(idx));
                            }
                        }
                        Statement::CompoundAssign { name, op, value } => {
                            if let Some(&idx) = locals.get(&name.name) {
                                function.instruction(&Instruction::LocalGet(idx));
                            }
                            self.compile_expr_with_locals(function, value, locals, locals_types)?;
                            match op {
                                BinOp::Add => { function.instruction(&Instruction::I32Add); }
                                BinOp::Sub => { function.instruction(&Instruction::I32Sub); }
                                _ => { function.instruction(&Instruction::I32Add); }
                            }
                            if let Some(&idx) = locals.get(&name.name) {
                                function.instruction(&Instruction::LocalSet(idx));
                            }
                        }
                        Statement::Let { name, value } => {
                            self.compile_expr_with_locals(function, value, locals, locals_types)?;
                            if let Some(&idx) = locals.get(&name.name) {
                                function.instruction(&Instruction::LocalSet(idx));
                            }
                        }
                        Statement::Return(opt_e) => {
                            if let Some(e) = opt_e {
                                self.compile_expr_with_locals(function, e, locals, locals_types)?;
                            }
                            function.instruction(&Instruction::Return);
                        }
                        Statement::Break { condition } => {
                            if let Some(cond) = condition {
                                self.compile_expr_with_locals(
                                    function,
                                    cond,
                                    locals,
                                    locals_types,
                                )?;
                                function.instruction(&Instruction::BrIf(1));
                            } else {
                                function.instruction(&Instruction::Br(1));
                            }
                        }
                        Statement::Continue { condition } => {
                            if let Some(cond) = condition {
                                self.compile_expr_with_locals(
                                    function,
                                    cond,
                                    locals,
                                    locals_types,
                                )?;
                                function.instruction(&Instruction::BrIf(0));
                            } else {
                                function.instruction(&Instruction::Br(0));
                            }
                        }
                        Statement::SharedLet { .. } => {}
                    }
                }

                // Increment index
                function.instruction(&Instruction::LocalGet(idx_local));
                function.instruction(&Instruction::I32Const(1));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalSet(idx_local));

                // Branch back to loop start
                function.instruction(&Instruction::Br(0));
                function.instruction(&Instruction::End); // end loop
                function.instruction(&Instruction::End); // end block

                // Restore loop depths
                self.loop_break_depth = prev_break;
                self.loop_continue_depth = prev_continue;

                // For-each returns unit (push 0)
                function.instruction(&Instruction::I32Const(0));
            }
            Expr::OptionalChain { expr, field, .. } => {
                // Optional chaining: expr?.field
                // If expr is some(v), return some(v.field)
                // If expr is none, return none
                //
                // For now: just compile expr and access field
                // Full implementation needs discriminant checking
                self.compile_expr_with_locals(function, expr, locals, locals_types)?;
                // Load field - simplified: assume it's at offset 4 (after discriminant)
                // Real implementation would need type info
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Add);
                // Access the field - TODO: need proper field offset calculation
                let _ = field;
                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Expr::Try { expr, .. } => {
                // Try operator: expr?
                // If expr is none/err, early return
                // If expr is some/ok, unwrap the value
                //
                // For now: just compile expr and unwrap
                // Full implementation needs discriminant check and early return
                self.compile_expr_with_locals(function, expr, locals, locals_types)?;
                // Assume some/ok discriminant is 1 - load payload at offset 4
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Expr::Await { expr, .. } => {
                // Await: suspend until future/async call completes
                //
                // With wasip3: Use the async import ABI
                // 1. Call the async function - returns status (i32)
                // 2. Check status:
                //    - Low 4 bits == 0: Not started, subtask index in high 28 bits
                //    - Low 4 bits == 1: Started but blocked
                //    - Low 4 bits == 2: Completed synchronously
                // 3. For blocked: add subtask to waitable-set, wait, then get result
                //
                // Component Model async status codes:
                //   CALL_NOT_STARTED = 0  (subtask_idx << 4)
                //   CALL_STARTED = 1
                //   CALL_RETURNED = 2
                if self.options.wasip3 {
                    // Compile inner expression (the async call) - returns status code
                    self.compile_expr_with_locals(function, expr, locals, locals_types)?;

                    // Get waitable-set imports
                    let ws_new_idx = self.ensure_waitable_set_new_import();
                    let ws_wait_idx = self.ensure_waitable_set_wait_import();
                    let subtask_drop_idx = self.ensure_subtask_drop_import();

                    // Stack: [status]
                    // Store status in temp local (use a high local index for temp storage)
                    let status_local = locals.len() as u32 + 10;
                    function.instruction(&Instruction::LocalSet(status_local));

                    // Check if completed synchronously: (status & 0xF) == 2
                    function.instruction(&Instruction::LocalGet(status_local));
                    function.instruction(&Instruction::I32Const(0xF));
                    function.instruction(&Instruction::I32And);
                    function.instruction(&Instruction::I32Const(2));
                    function.instruction(&Instruction::I32Eq);

                    // Result block - pushes i32 result onto stack
                    function.instruction(&Instruction::If(wasm_encoder::BlockType::Result(
                        ValType::I32,
                    )));

                    // Completed synchronously - result at out-ptr (offset 0)
                    function.instruction(&Instruction::I32Const(0));
                    function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                        offset: 0,
                        align: 2,
                        memory_index: 0,
                    }));

                    function.instruction(&Instruction::Else);

                    // Not completed - need to wait
                    // Create waitable-set
                    function.instruction(&Instruction::Call(ws_new_idx));
                    let ws_local = status_local + 1;
                    function.instruction(&Instruction::LocalSet(ws_local));

                    // Wait loop: call waitable-set.wait until subtask completes
                    function.instruction(&Instruction::Block(wasm_encoder::BlockType::Empty));
                    function.instruction(&Instruction::Loop(wasm_encoder::BlockType::Empty));

                    // Call waitable-set.wait(ws, out_ptr)
                    function.instruction(&Instruction::LocalGet(ws_local));
                    function.instruction(&Instruction::I32Const(0)); // out_ptr at offset 0
                    function.instruction(&Instruction::Call(ws_wait_idx));

                    // Check returned event status (2 = RETURNED means done)
                    function.instruction(&Instruction::I32Const(0xF));
                    function.instruction(&Instruction::I32And);
                    function.instruction(&Instruction::I32Const(2));
                    function.instruction(&Instruction::I32Eq);
                    function.instruction(&Instruction::BrIf(1)); // break out if done

                    // Not done, loop again
                    function.instruction(&Instruction::Br(0));
                    function.instruction(&Instruction::End); // loop
                    function.instruction(&Instruction::End); // block

                    // Clean up: drop subtask (get index from original status high 28 bits)
                    function.instruction(&Instruction::LocalGet(status_local));
                    function.instruction(&Instruction::I32Const(4));
                    function.instruction(&Instruction::I32ShrU);
                    function.instruction(&Instruction::Call(subtask_drop_idx));

                    // Load result from out-ptr
                    function.instruction(&Instruction::I32Const(0));
                    function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                        offset: 0,
                        align: 2,
                        memory_index: 0,
                    }));

                    function.instruction(&Instruction::End); // if/else
                } else if self.options.threads {
                    // Thread-await: await tid → thread.join(tid)
                    // Compile tid (produces flag_offset)
                    self.compile_expr_with_locals(function, expr, locals, locals_types)?;
                    // memory.atomic.wait32(flag_offset, expected=0, timeout=-1)
                    function.instruction(&Instruction::I32Const(0)); // expected
                    function.instruction(&Instruction::I64Const(-1)); // infinite timeout
                    function.instruction(&Instruction::MemoryAtomicWait32(wasm_encoder::MemArg {
                        offset: 0,
                        align: 2,
                        memory_index: 0,
                    }));
                    function.instruction(&Instruction::Drop); // discard wait result
                    function.instruction(&Instruction::I32Const(0)); // push unit
                } else {
                    // Sync ABI - just evaluate the expression directly
                    self.compile_expr_with_locals(function, expr, locals, locals_types)?;
                }
            }
            Expr::Spawn { body, .. } => {
                // Allocate a done-flag in shared memory (4-byte aligned)
                let flag_offset = self.string_offset;
                self.string_offset += 4;
                // Initialize flag to 0 (not done)
                function.instruction(&Instruction::I32Const(flag_offset as i32));
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::I32AtomicStore(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));

                let spawn_id = self.next_spawn_id;
                self.next_spawn_id += 1;
                let func_idx = 0u32;
                self.spawn_bodies.push((spawn_id, func_idx, body.clone()));
                let thread_spawn_idx = self.ensure_thread_spawn_import();
                function.instruction(&Instruction::I32Const(spawn_id as i32));
                function.instruction(&Instruction::Call(thread_spawn_idx));
                function.instruction(&Instruction::Drop); // discard thread-spawn return

                // Push flag_offset as the tid (used by thread.join)
                function.instruction(&Instruction::I32Const(flag_offset as i32));
            }
            Expr::ThreadJoin { tid, .. } => {
                // Compile tid expression (produces flag_offset)
                self.compile_expr_with_locals(function, tid, locals, locals_types)?;
                // memory.atomic.wait32(flag_offset, expected=0, timeout=-1)
                // Blocks until flag != 0 (i.e., until spawned thread sets it to 1)
                function.instruction(&Instruction::I32Const(0)); // expected value
                function.instruction(&Instruction::I64Const(-1)); // infinite timeout
                function.instruction(&Instruction::MemoryAtomicWait32(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                function.instruction(&Instruction::Drop); // discard wait result
                function.instruction(&Instruction::I32Const(0)); // push unit result
            }
            Expr::AtomicLoad { addr, .. } => {
                self.compile_expr_with_locals(function, addr, locals, locals_types)?;
                function.instruction(&Instruction::I32AtomicLoad(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Expr::AtomicStore { addr, value, .. } => {
                self.compile_expr_with_locals(function, addr, locals, locals_types)?;
                self.compile_expr_with_locals(function, value, locals, locals_types)?;
                function.instruction(&Instruction::I32AtomicStore(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                function.instruction(&Instruction::I32Const(0)); // store returns nothing, push dummy
            }
            Expr::AtomicAdd { addr, value, .. } => {
                self.compile_expr_with_locals(function, addr, locals, locals_types)?;
                self.compile_expr_with_locals(function, value, locals, locals_types)?;
                function.instruction(&Instruction::I32AtomicRmwAdd(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Expr::AtomicSub { addr, value, .. } => {
                self.compile_expr_with_locals(function, addr, locals, locals_types)?;
                self.compile_expr_with_locals(function, value, locals, locals_types)?;
                function.instruction(&Instruction::I32AtomicRmwSub(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Expr::AtomicCmpxchg { addr, expected, replacement, .. } => {
                self.compile_expr_with_locals(function, addr, locals, locals_types)?;
                self.compile_expr_with_locals(function, expected, locals, locals_types)?;
                self.compile_expr_with_locals(function, replacement, locals, locals_types)?;
                function.instruction(&Instruction::I32AtomicRmwCmpxchg(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Expr::AtomicWait { addr, expected, timeout, .. } => {
                self.compile_expr_with_locals(function, addr, locals, locals_types)?;
                self.compile_expr_with_locals(function, expected, locals, locals_types)?;
                self.compile_expr_with_locals(function, timeout, locals, locals_types)?;
                function.instruction(&Instruction::MemoryAtomicWait32(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Expr::AtomicNotify { addr, count, .. } => {
                self.compile_expr_with_locals(function, addr, locals, locals_types)?;
                self.compile_expr_with_locals(function, count, locals, locals_types)?;
                function.instruction(&Instruction::MemoryAtomicNotify(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Expr::AtomicBlock { body, .. } => {
                if body.is_empty() {
                    function.instruction(&Instruction::I32Const(0));
                } else {
                    let mut locals_types_mut = locals_types.clone();
                    for stmt in &body[..body.len() - 1] {
                        self.compile_atomic_statement(function, stmt, locals, &mut locals_types_mut)?;
                    }
                    match &body[body.len() - 1] {
                        Statement::Expr(expr) => {
                            self.compile_atomic_expr(function, expr, locals, locals_types)?;
                        }
                        _ => {
                            self.compile_atomic_statement(function, &body[body.len() - 1], locals, &mut locals_types_mut)?;
                            function.instruction(&Instruction::I32Const(0));
                        }
                    }
                }
            }
            Expr::SimdOp { lane, op, args, lane_idx, .. } => {
                self.compile_simd_op(function, lane, op, args, *lane_idx, locals, locals_types)?;
            }
            Expr::SimdForEach { variable, collection, body, .. } => {
                // SIMD for-each: simd for v in list { body }
                // Processes list elements 4-at-a-time using v128 load/store.
                //
                // Codegen pattern:
                //   list_ptr = compile(collection)
                //   length = i32.load(list_ptr)
                //   end = (length / 4) * 4
                //   idx = 0
                //   block $break
                //     loop $continue
                //       if idx >= end: br $break
                //       v = v128.load(list_ptr + 4 + idx*4)
                //       result = body(v)          // user SIMD ops on v
                //       v128.store(list_ptr + 4 + idx*4, result)
                //       idx += 4
                //       br $continue
                //     end
                //   end

                // Allocate temp locals: list_ptr, idx, end
                let list_ptr_local = locals.len() as u32;
                let idx_local = list_ptr_local + 1;
                let end_local = idx_local + 1;
                let v_local = *locals
                    .get(&variable.name)
                    .expect("simd for-each variable not found");

                // Compile collection and store pointer
                self.compile_expr_with_locals(function, collection, locals, locals_types)?;
                function.instruction(&Instruction::LocalSet(list_ptr_local));

                // end = (i32.load(list_ptr) / 4) * 4
                function.instruction(&Instruction::LocalGet(list_ptr_local));
                function.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32DivU);
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Mul);
                function.instruction(&Instruction::LocalSet(end_local));

                // idx = 0
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::LocalSet(idx_local));

                // block $break
                function.instruction(&Instruction::Block(wasm_encoder::BlockType::Empty));
                // loop $continue
                function.instruction(&Instruction::Loop(wasm_encoder::BlockType::Empty));

                // if idx >= end: br $break
                function.instruction(&Instruction::LocalGet(idx_local));
                function.instruction(&Instruction::LocalGet(end_local));
                function.instruction(&Instruction::I32GeS);
                function.instruction(&Instruction::BrIf(1)); // break to outer block

                // v = v128.load(list_ptr + 4 + idx*4)
                function.instruction(&Instruction::LocalGet(list_ptr_local));
                function.instruction(&Instruction::I32Const(4)); // skip length header
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalGet(idx_local));
                function.instruction(&Instruction::I32Const(4)); // element size
                function.instruction(&Instruction::I32Mul);
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::V128Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 4, // 16-byte aligned
                    memory_index: 0,
                }));
                function.instruction(&Instruction::LocalSet(v_local));

                // Compile body statements: last expression's v128 result stays on stack
                let mut has_result = false;
                for (i, stmt) in body.iter().enumerate() {
                    match stmt {
                        Statement::Expr(e) => {
                            self.compile_expr_with_locals(function, e, locals, locals_types)?;
                            if i < body.len() - 1 {
                                function.instruction(&Instruction::Drop);
                            } else {
                                has_result = true;
                            }
                        }
                        Statement::Assign { name, value } => {
                            self.compile_expr_with_locals(function, value, locals, locals_types)?;
                            if let Some(&idx) = locals.get(&name.name) {
                                function.instruction(&Instruction::LocalSet(idx));
                            }
                        }
                        Statement::Let { name, value } => {
                            self.compile_expr_with_locals(function, value, locals, locals_types)?;
                            if let Some(&idx) = locals.get(&name.name) {
                                function.instruction(&Instruction::LocalSet(idx));
                            }
                        }
                        Statement::CompoundAssign { name, op, value } => {
                            if let Some(&idx) = locals.get(&name.name) {
                                function.instruction(&Instruction::LocalGet(idx));
                            }
                            self.compile_expr_with_locals(function, value, locals, locals_types)?;
                            match op {
                                BinOp::Add => { function.instruction(&Instruction::I32Add); }
                                BinOp::Sub => { function.instruction(&Instruction::I32Sub); }
                                _ => { function.instruction(&Instruction::I32Add); }
                            }
                            if let Some(&idx) = locals.get(&name.name) {
                                function.instruction(&Instruction::LocalSet(idx));
                            }
                        }
                        Statement::Return(opt_e) => {
                            if let Some(e) = opt_e {
                                self.compile_expr_with_locals(function, e, locals, locals_types)?;
                            }
                            function.instruction(&Instruction::Return);
                        }
                        Statement::Break { .. } | Statement::Continue { .. }
                        | Statement::SharedLet { .. } => {}
                    }
                }

                if has_result {
                    // v128.store(list_ptr + 4 + idx*4, result)
                    // But v128.store expects (addr, value) — addr first.
                    // The result is on top of stack, so we need to compute addr first.
                    // Store result to a temp, compute addr, load result, then store.
                    let result_local = v_local; // reuse v_local as temp
                    function.instruction(&Instruction::LocalSet(result_local));

                    function.instruction(&Instruction::LocalGet(list_ptr_local));
                    function.instruction(&Instruction::I32Const(4));
                    function.instruction(&Instruction::I32Add);
                    function.instruction(&Instruction::LocalGet(idx_local));
                    function.instruction(&Instruction::I32Const(4));
                    function.instruction(&Instruction::I32Mul);
                    function.instruction(&Instruction::I32Add);
                    function.instruction(&Instruction::LocalGet(result_local));
                    function.instruction(&Instruction::V128Store(wasm_encoder::MemArg {
                        offset: 0,
                        align: 4,
                        memory_index: 0,
                    }));
                }

                // idx += 4
                function.instruction(&Instruction::LocalGet(idx_local));
                function.instruction(&Instruction::I32Const(4));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalSet(idx_local));

                // br $continue (loop back)
                function.instruction(&Instruction::Br(0));

                // end loop
                function.instruction(&Instruction::End);
                // end block
                function.instruction(&Instruction::End);
            }
        }
        Ok(())
    }

    /// Compile a SIMD operation to WASM SIMD instructions.
    #[allow(clippy::too_many_arguments)]
    fn compile_simd_op(
        &mut self,
        function: &mut Function,
        lane: &SimdLane,
        op: &SimdOp,
        args: &[Expr],
        lane_idx: Option<u8>,
        locals: &HashMap<String, u32>,
        locals_types: &HashMap<String, RecordTypeInfo>,
    ) -> Result<(), CompileError> {
        use SimdLane::*;
        use SimdOp::*;

        // Compile all arguments onto the stack
        for arg in args {
            self.compile_expr_with_locals(function, arg, locals, locals_types)?;
        }

        let memarg = wasm_encoder::MemArg { offset: 0, align: 4, memory_index: 0 };
        let lidx = lane_idx.unwrap_or(0) as u8;

        match (lane, op) {
            // === Splat ===
            (I8x16, Splat) => { function.instruction(&Instruction::I8x16Splat); }
            (I16x8, Splat) => { function.instruction(&Instruction::I16x8Splat); }
            (I32x4, Splat) => { function.instruction(&Instruction::I32x4Splat); }
            (I64x2, Splat) => { function.instruction(&Instruction::I64x2Splat); }
            (F32x4, Splat) => { function.instruction(&Instruction::F32x4Splat); }
            (F64x2, Splat) => { function.instruction(&Instruction::F64x2Splat); }

            // === Add ===
            (I8x16, Add) => { function.instruction(&Instruction::I8x16Add); }
            (I16x8, Add) => { function.instruction(&Instruction::I16x8Add); }
            (I32x4, Add) => { function.instruction(&Instruction::I32x4Add); }
            (I64x2, Add) => { function.instruction(&Instruction::I64x2Add); }
            (F32x4, Add) => { function.instruction(&Instruction::F32x4Add); }
            (F64x2, Add) => { function.instruction(&Instruction::F64x2Add); }

            // === Sub ===
            (I8x16, Sub) => { function.instruction(&Instruction::I8x16Sub); }
            (I16x8, Sub) => { function.instruction(&Instruction::I16x8Sub); }
            (I32x4, Sub) => { function.instruction(&Instruction::I32x4Sub); }
            (I64x2, Sub) => { function.instruction(&Instruction::I64x2Sub); }
            (F32x4, Sub) => { function.instruction(&Instruction::F32x4Sub); }
            (F64x2, Sub) => { function.instruction(&Instruction::F64x2Sub); }

            // === Mul ===
            (I16x8, Mul) => { function.instruction(&Instruction::I16x8Mul); }
            (I32x4, Mul) => { function.instruction(&Instruction::I32x4Mul); }
            (I64x2, Mul) => { function.instruction(&Instruction::I64x2Mul); }
            (F32x4, Mul) => { function.instruction(&Instruction::F32x4Mul); }
            (F64x2, Mul) => { function.instruction(&Instruction::F64x2Mul); }

            // === Neg ===
            (I8x16, Neg) => { function.instruction(&Instruction::I8x16Neg); }
            (I16x8, Neg) => { function.instruction(&Instruction::I16x8Neg); }
            (I32x4, Neg) => { function.instruction(&Instruction::I32x4Neg); }
            (I64x2, Neg) => { function.instruction(&Instruction::I64x2Neg); }
            (F32x4, Neg) => { function.instruction(&Instruction::F32x4Neg); }
            (F64x2, Neg) => { function.instruction(&Instruction::F64x2Neg); }

            // === Abs ===
            (I8x16, Abs) => { function.instruction(&Instruction::I8x16Abs); }
            (I16x8, Abs) => { function.instruction(&Instruction::I16x8Abs); }
            (I32x4, Abs) => { function.instruction(&Instruction::I32x4Abs); }
            (I64x2, Abs) => { function.instruction(&Instruction::I64x2Abs); }
            (F32x4, Abs) => { function.instruction(&Instruction::F32x4Abs); }
            (F64x2, Abs) => { function.instruction(&Instruction::F64x2Abs); }

            // === Float-only: Div, Sqrt, Ceil, Floor, Trunc, Nearest ===
            (F32x4, Div) => { function.instruction(&Instruction::F32x4Div); }
            (F64x2, Div) => { function.instruction(&Instruction::F64x2Div); }
            (F32x4, Sqrt) => { function.instruction(&Instruction::F32x4Sqrt); }
            (F64x2, Sqrt) => { function.instruction(&Instruction::F64x2Sqrt); }
            (F32x4, Ceil) => { function.instruction(&Instruction::F32x4Ceil); }
            (F64x2, Ceil) => { function.instruction(&Instruction::F64x2Ceil); }
            (F32x4, Floor) => { function.instruction(&Instruction::F32x4Floor); }
            (F64x2, Floor) => { function.instruction(&Instruction::F64x2Floor); }
            (F32x4, Trunc) => { function.instruction(&Instruction::F32x4Trunc); }
            (F64x2, Trunc) => { function.instruction(&Instruction::F64x2Trunc); }
            (F32x4, Nearest) => { function.instruction(&Instruction::F32x4Nearest); }
            (F64x2, Nearest) => { function.instruction(&Instruction::F64x2Nearest); }

            // === Shifts ===
            (I8x16, Shl) => { function.instruction(&Instruction::I8x16Shl); }
            (I16x8, Shl) => { function.instruction(&Instruction::I16x8Shl); }
            (I32x4, Shl) => { function.instruction(&Instruction::I32x4Shl); }
            (I64x2, Shl) => { function.instruction(&Instruction::I64x2Shl); }
            (I8x16, ShrS) => { function.instruction(&Instruction::I8x16ShrS); }
            (I16x8, ShrS) => { function.instruction(&Instruction::I16x8ShrS); }
            (I32x4, ShrS) => { function.instruction(&Instruction::I32x4ShrS); }
            (I64x2, ShrS) => { function.instruction(&Instruction::I64x2ShrS); }
            (I8x16, ShrU) => { function.instruction(&Instruction::I8x16ShrU); }
            (I16x8, ShrU) => { function.instruction(&Instruction::I16x8ShrU); }
            (I32x4, ShrU) => { function.instruction(&Instruction::I32x4ShrU); }
            (I64x2, ShrU) => { function.instruction(&Instruction::I64x2ShrU); }

            // === Min / Max ===
            (I8x16, Min) => { function.instruction(&Instruction::I8x16MinS); }
            (I16x8, Min) => { function.instruction(&Instruction::I16x8MinS); }
            (I32x4, Min) => { function.instruction(&Instruction::I32x4MinS); }
            (F32x4, Min) => { function.instruction(&Instruction::F32x4Min); }
            (F64x2, Min) => { function.instruction(&Instruction::F64x2Min); }
            (I8x16, Max) => { function.instruction(&Instruction::I8x16MaxS); }
            (I16x8, Max) => { function.instruction(&Instruction::I16x8MaxS); }
            (I32x4, Max) => { function.instruction(&Instruction::I32x4MaxS); }
            (F32x4, Max) => { function.instruction(&Instruction::F32x4Max); }
            (F64x2, Max) => { function.instruction(&Instruction::F64x2Max); }

            // === Extract / Replace Lane ===
            (I8x16, ExtractLane) => { function.instruction(&Instruction::I8x16ExtractLaneS(lidx)); }
            (I16x8, ExtractLane) => { function.instruction(&Instruction::I16x8ExtractLaneS(lidx)); }
            (I32x4, ExtractLane) => { function.instruction(&Instruction::I32x4ExtractLane(lidx)); }
            (I64x2, ExtractLane) => { function.instruction(&Instruction::I64x2ExtractLane(lidx)); }
            (F32x4, ExtractLane) => { function.instruction(&Instruction::F32x4ExtractLane(lidx)); }
            (F64x2, ExtractLane) => { function.instruction(&Instruction::F64x2ExtractLane(lidx)); }
            (I8x16, ReplaceLane) => { function.instruction(&Instruction::I8x16ReplaceLane(lidx)); }
            (I16x8, ReplaceLane) => { function.instruction(&Instruction::I16x8ReplaceLane(lidx)); }
            (I32x4, ReplaceLane) => { function.instruction(&Instruction::I32x4ReplaceLane(lidx)); }
            (I64x2, ReplaceLane) => { function.instruction(&Instruction::I64x2ReplaceLane(lidx)); }
            (F32x4, ReplaceLane) => { function.instruction(&Instruction::F32x4ReplaceLane(lidx)); }
            (F64x2, ReplaceLane) => { function.instruction(&Instruction::F64x2ReplaceLane(lidx)); }

            // === Comparisons (integer signed) ===
            (I8x16, Eq) => { function.instruction(&Instruction::I8x16Eq); }
            (I16x8, Eq) => { function.instruction(&Instruction::I16x8Eq); }
            (I32x4, Eq) => { function.instruction(&Instruction::I32x4Eq); }
            (I64x2, Eq) => { function.instruction(&Instruction::I64x2Eq); }
            (I8x16, Ne) => { function.instruction(&Instruction::I8x16Ne); }
            (I16x8, Ne) => { function.instruction(&Instruction::I16x8Ne); }
            (I32x4, Ne) => { function.instruction(&Instruction::I32x4Ne); }
            (I64x2, Ne) => { function.instruction(&Instruction::I64x2Ne); }
            (I8x16, LtS) => { function.instruction(&Instruction::I8x16LtS); }
            (I16x8, LtS) => { function.instruction(&Instruction::I16x8LtS); }
            (I32x4, LtS) => { function.instruction(&Instruction::I32x4LtS); }
            (I64x2, LtS) => { function.instruction(&Instruction::I64x2LtS); }
            (I8x16, GtS) => { function.instruction(&Instruction::I8x16GtS); }
            (I16x8, GtS) => { function.instruction(&Instruction::I16x8GtS); }
            (I32x4, GtS) => { function.instruction(&Instruction::I32x4GtS); }
            (I64x2, GtS) => { function.instruction(&Instruction::I64x2GtS); }
            (I8x16, LeS) => { function.instruction(&Instruction::I8x16LeS); }
            (I16x8, LeS) => { function.instruction(&Instruction::I16x8LeS); }
            (I32x4, LeS) => { function.instruction(&Instruction::I32x4LeS); }
            (I64x2, LeS) => { function.instruction(&Instruction::I64x2LeS); }
            (I8x16, GeS) => { function.instruction(&Instruction::I8x16GeS); }
            (I16x8, GeS) => { function.instruction(&Instruction::I16x8GeS); }
            (I32x4, GeS) => { function.instruction(&Instruction::I32x4GeS); }
            (I64x2, GeS) => { function.instruction(&Instruction::I64x2GeS); }

            // === Comparisons (unsigned) ===
            (I8x16, LtU) => { function.instruction(&Instruction::I8x16LtU); }
            (I16x8, LtU) => { function.instruction(&Instruction::I16x8LtU); }
            (I32x4, LtU) => { function.instruction(&Instruction::I32x4LtU); }
            (I8x16, GtU) => { function.instruction(&Instruction::I8x16GtU); }
            (I16x8, GtU) => { function.instruction(&Instruction::I16x8GtU); }
            (I32x4, GtU) => { function.instruction(&Instruction::I32x4GtU); }
            (I8x16, LeU) => { function.instruction(&Instruction::I8x16LeU); }
            (I16x8, LeU) => { function.instruction(&Instruction::I16x8LeU); }
            (I32x4, LeU) => { function.instruction(&Instruction::I32x4LeU); }
            (I8x16, GeU) => { function.instruction(&Instruction::I8x16GeU); }
            (I16x8, GeU) => { function.instruction(&Instruction::I16x8GeU); }
            (I32x4, GeU) => { function.instruction(&Instruction::I32x4GeU); }

            // === Float comparisons ===
            (F32x4, Eq) | (F32x4, Lt) | (F32x4, Gt) | (F32x4, Le) | (F32x4, Ge) | (F32x4, Ne) => {
                let instr = match op {
                    Eq => Instruction::F32x4Eq,
                    Ne => Instruction::F32x4Ne,
                    Lt => Instruction::F32x4Lt,
                    Gt => Instruction::F32x4Gt,
                    Le => Instruction::F32x4Le,
                    Ge => Instruction::F32x4Ge,
                    _ => unreachable!(),
                };
                function.instruction(&instr);
            }
            (F64x2, Eq) | (F64x2, Lt) | (F64x2, Gt) | (F64x2, Le) | (F64x2, Ge) | (F64x2, Ne) => {
                let instr = match op {
                    Eq => Instruction::F64x2Eq,
                    Ne => Instruction::F64x2Ne,
                    Lt => Instruction::F64x2Lt,
                    Gt => Instruction::F64x2Gt,
                    Le => Instruction::F64x2Le,
                    Ge => Instruction::F64x2Ge,
                    _ => unreachable!(),
                };
                function.instruction(&instr);
            }

            // === Bitwise (v128 interpretation) ===
            (_, And) => { function.instruction(&Instruction::V128And); }
            (_, Or) => { function.instruction(&Instruction::V128Or); }
            (_, Xor) => { function.instruction(&Instruction::V128Xor); }
            (_, Not) => { function.instruction(&Instruction::V128Not); }
            (_, AndNot) => { function.instruction(&Instruction::V128AndNot); }
            (_, Bitselect) => { function.instruction(&Instruction::V128Bitselect); }

            // === Tests ===
            (_, AnyTrue) => { function.instruction(&Instruction::V128AnyTrue); }
            (I8x16, AllTrue) => { function.instruction(&Instruction::I8x16AllTrue); }
            (I16x8, AllTrue) => { function.instruction(&Instruction::I16x8AllTrue); }
            (I32x4, AllTrue) => { function.instruction(&Instruction::I32x4AllTrue); }
            (I64x2, AllTrue) => { function.instruction(&Instruction::I64x2AllTrue); }
            (I8x16, Bitmask) => { function.instruction(&Instruction::I8x16Bitmask); }
            (I16x8, Bitmask) => { function.instruction(&Instruction::I16x8Bitmask); }
            (I32x4, Bitmask) => { function.instruction(&Instruction::I32x4Bitmask); }
            (I64x2, Bitmask) => { function.instruction(&Instruction::I64x2Bitmask); }

            // === Swizzle ===
            (I8x16, Swizzle) => { function.instruction(&Instruction::I8x16Swizzle); }

            // === Memory ===
            (_, Load) => { function.instruction(&Instruction::V128Load(memarg)); }
            (_, Store) => { function.instruction(&Instruction::V128Store(memarg)); }

            // === Popcnt ===
            (I8x16, Popcnt) => { function.instruction(&Instruction::I8x16Popcnt); }

            // === AvgR ===
            (I8x16, AvgRU) => { function.instruction(&Instruction::I8x16AvgrU); }
            (I16x8, AvgRU) => { function.instruction(&Instruction::I16x8AvgrU); }

            // === Dot ===
            (I32x4, Dot) => { function.instruction(&Instruction::I32x4DotI16x8S); }

            // === Widening multiply ===
            (I16x8, ExtMulLowS) => { function.instruction(&Instruction::I16x8ExtMulLowI8x16S); }
            (I16x8, ExtMulHighS) => { function.instruction(&Instruction::I16x8ExtMulHighI8x16S); }
            (I16x8, ExtMulLowU) => { function.instruction(&Instruction::I16x8ExtMulLowI8x16U); }
            (I16x8, ExtMulHighU) => { function.instruction(&Instruction::I16x8ExtMulHighI8x16U); }
            (I32x4, ExtMulLowS) => { function.instruction(&Instruction::I32x4ExtMulLowI16x8S); }
            (I32x4, ExtMulHighS) => { function.instruction(&Instruction::I32x4ExtMulHighI16x8S); }
            (I32x4, ExtMulLowU) => { function.instruction(&Instruction::I32x4ExtMulLowI16x8U); }
            (I32x4, ExtMulHighU) => { function.instruction(&Instruction::I32x4ExtMulHighI16x8U); }
            (I64x2, ExtMulLowS) => { function.instruction(&Instruction::I64x2ExtMulLowI32x4S); }
            (I64x2, ExtMulHighS) => { function.instruction(&Instruction::I64x2ExtMulHighI32x4S); }
            (I64x2, ExtMulLowU) => { function.instruction(&Instruction::I64x2ExtMulLowI32x4U); }
            (I64x2, ExtMulHighU) => { function.instruction(&Instruction::I64x2ExtMulHighI32x4U); }

            // === Pairwise add ===
            (I16x8, ExtAddPairwiseS) => { function.instruction(&Instruction::I16x8ExtAddPairwiseI8x16S); }
            (I16x8, ExtAddPairwiseU) => { function.instruction(&Instruction::I16x8ExtAddPairwiseI8x16U); }
            (I32x4, ExtAddPairwiseS) => { function.instruction(&Instruction::I32x4ExtAddPairwiseI16x8S); }
            (I32x4, ExtAddPairwiseU) => { function.instruction(&Instruction::I32x4ExtAddPairwiseI16x8U); }

            // === Narrowing ===
            (I8x16, NarrowS) => { function.instruction(&Instruction::I8x16NarrowI16x8S); }
            (I8x16, NarrowU) => { function.instruction(&Instruction::I8x16NarrowI16x8U); }
            (I16x8, NarrowS) => { function.instruction(&Instruction::I16x8NarrowI32x4S); }
            (I16x8, NarrowU) => { function.instruction(&Instruction::I16x8NarrowI32x4U); }

            // === Extending ===
            (I16x8, ExtendLowS) => { function.instruction(&Instruction::I16x8ExtendLowI8x16S); }
            (I16x8, ExtendHighS) => { function.instruction(&Instruction::I16x8ExtendHighI8x16S); }
            (I16x8, ExtendLowU) => { function.instruction(&Instruction::I16x8ExtendLowI8x16U); }
            (I16x8, ExtendHighU) => { function.instruction(&Instruction::I16x8ExtendHighI8x16U); }
            (I32x4, ExtendLowS) => { function.instruction(&Instruction::I32x4ExtendLowI16x8S); }
            (I32x4, ExtendHighS) => { function.instruction(&Instruction::I32x4ExtendHighI16x8S); }
            (I32x4, ExtendLowU) => { function.instruction(&Instruction::I32x4ExtendLowI16x8U); }
            (I32x4, ExtendHighU) => { function.instruction(&Instruction::I32x4ExtendHighI16x8U); }
            (I64x2, ExtendLowS) => { function.instruction(&Instruction::I64x2ExtendLowI32x4S); }
            (I64x2, ExtendHighS) => { function.instruction(&Instruction::I64x2ExtendHighI32x4S); }
            (I64x2, ExtendLowU) => { function.instruction(&Instruction::I64x2ExtendLowI32x4U); }
            (I64x2, ExtendHighU) => { function.instruction(&Instruction::I64x2ExtendHighI32x4U); }

            // Fallback: unsupported combination — emit i32.const 0
            _ => {
                function.instruction(&Instruction::I32Const(0));
            }
        }
        Ok(())
    }

    /// Compile a statement inside an `atomic { }` block.
    /// Shared variables are desugared to atomic WASM instructions.
    fn compile_atomic_statement(
        &mut self,
        function: &mut Function,
        stmt: &Statement,
        locals: &HashMap<String, u32>,
        locals_types: &mut HashMap<String, RecordTypeInfo>,
    ) -> Result<(), CompileError> {
        match stmt {
            Statement::CompoundAssign { name, op, value } if self.shared_locals.contains(&name.name) => {
                // Desugar: shared_var += val → i32.atomic.rmw.add(offset, val)
                if let Some(&idx) = locals.get(&name.name) {
                    function.instruction(&Instruction::LocalGet(idx)); // push memory offset
                }
                self.compile_expr_with_locals(function, value, locals, locals_types)?;
                let memarg = wasm_encoder::MemArg { offset: 0, align: 2, memory_index: 0 };
                match op {
                    BinOp::Add => { function.instruction(&Instruction::I32AtomicRmwAdd(memarg)); }
                    BinOp::Sub => { function.instruction(&Instruction::I32AtomicRmwSub(memarg)); }
                    _ => { function.instruction(&Instruction::I32AtomicRmwAdd(memarg)); }
                }
                function.instruction(&Instruction::Drop); // rmw returns old value; discard
            }
            Statement::Assign { name, value } if self.shared_locals.contains(&name.name) => {
                // Desugar: shared_var = val → i32.atomic.store(offset, val)
                if let Some(&idx) = locals.get(&name.name) {
                    function.instruction(&Instruction::LocalGet(idx)); // push memory offset
                }
                self.compile_expr_with_locals(function, value, locals, locals_types)?;
                function.instruction(&Instruction::I32AtomicStore(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
            }
            // Non-shared: fall through to regular compilation
            _ => {
                self.compile_statement_with_locals(function, stmt, locals, locals_types)?;
            }
        }
        Ok(())
    }

    /// Compile an expression inside an `atomic { }` block.
    /// Bare references to shared variables desugar to atomic loads.
    fn compile_atomic_expr(
        &mut self,
        function: &mut Function,
        expr: &Expr,
        locals: &HashMap<String, u32>,
        locals_types: &HashMap<String, RecordTypeInfo>,
    ) -> Result<(), CompileError> {
        match expr {
            Expr::Ident(id) if self.shared_locals.contains(&id.name) => {
                // Desugar: shared_var → i32.atomic.load(offset)
                if let Some(&idx) = locals.get(&id.name) {
                    function.instruction(&Instruction::LocalGet(idx)); // push memory offset
                }
                function.instruction(&Instruction::I32AtomicLoad(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
            }
            // Non-shared: fall through to regular compilation
            _ => {
                self.compile_expr_with_locals(function, expr, locals, locals_types)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kettu_parser::parse_file;

    #[test]
    fn test_compile_simple_function() {
        let source = r#"
            package local:test;
            
            interface host {
                log: func(msg: string) {
                    println(msg)
                }
            }
        "#;

        let (ast, errors) = parse_file(source);
        for e in &errors {
            eprintln!("Parse error: {:?}", e);
        }
        let ast = ast.expect("Should parse");

        let options = CompileOptions::default();
        let wasm = compile_module(&ast, &options).expect("Should compile");

        // Verify it's valid WASM
        assert!(wasm.starts_with(&[0x00, 0x61, 0x73, 0x6d])); // WASM magic
        assert!(wasm.len() > 8);
    }

    #[test]
    fn test_compile_with_locals() {
        let source = r#"
            package local:test;
            
            interface math {
                add-one: func(x: s32) -> s32 {
                    let y = x + 1;
                    return y;
                }
            }
        "#;

        let (ast, errors) = parse_file(source);
        for e in &errors {
            eprintln!("Parse error: {:?}", e);
        }
        let ast = ast.expect("Should parse");

        let options = CompileOptions::default();
        let wasm = compile_module(&ast, &options).expect("Should compile");

        // Verify it's valid WASM
        assert!(wasm.starts_with(&[0x00, 0x61, 0x73, 0x6d]));
        assert!(wasm.len() > 8);
    }

    #[test]
    fn test_compile_binary_operators() {
        let source = r#"
            package local:test;
            
            interface ops {
                compute: func(a: s32, b: s32) -> s32 {
                    let sum = a + b;
                    let diff = a - b;
                    let prod = a * b;
                    return prod;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");

        let options = CompileOptions::default();
        let wasm = compile_module(&ast, &options).expect("Should compile");

        assert!(wasm.starts_with(&[0x00, 0x61, 0x73, 0x6d]));
    }

    #[test]
    fn test_compile_shared_let() {
        let source = r#"
            package local:test;

            interface effects {
                go: func() -> s32 {
                    shared let counter = 0;
                    0;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");

        let options = CompileOptions::default();
        let wasm = compile_module(&ast, &options).expect("shared let should compile");
        assert!(wasm.starts_with(&[0x00, 0x61, 0x73, 0x6d]));
    }

    #[test]
    fn test_compile_atomic_block() {
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

        let options = CompileOptions::default();
        let wasm = compile_module(&ast, &options).expect("atomic block should compile");
        assert!(wasm.starts_with(&[0x00, 0x61, 0x73, 0x6d]));
    }

    #[test]
    fn test_compile_shared_let_with_atomic_block() {
        let source = r#"
            package local:test;

            interface effects {
                inc: func() -> s32 {
                    shared let counter = 0;
                    atomic {
                        1;
                    };
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");

        let options = CompileOptions::default();
        let wasm = compile_module(&ast, &options).expect("shared let + atomic should compile");
        assert!(wasm.starts_with(&[0x00, 0x61, 0x73, 0x6d]));
    }

    #[test]
    fn test_compile_thread_join() {
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

        let options = CompileOptions { threads: true, ..Default::default() };
        let wasm = compile_module(&ast, &options).expect("thread.join should compile");
        assert!(wasm.starts_with(&[0x00, 0x61, 0x73, 0x6d]));
    }

    #[test]
    fn test_compile_spawn_join_full() {
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

        let options = CompileOptions { threads: true, ..Default::default() };
        let wasm = compile_module(&ast, &options).expect("full spawn+join should compile");
        assert!(wasm.starts_with(&[0x00, 0x61, 0x73, 0x6d]));
    }

    #[test]
    fn test_compile_compound_assign() {
        let source = r#"
            package local:test;

            interface effects {
                go: func() -> s32 {
                    let x = 0;
                    x += 5;
                    x -= 2;
                    x;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");

        let options = CompileOptions::default();
        let wasm = compile_module(&ast, &options).expect("compound assign should compile");
        assert!(wasm.starts_with(&[0x00, 0x61, 0x73, 0x6d]));
    }

    #[test]
    fn test_compile_atomic_block_desugaring() {
        let source = r#"
            package local:test;

            interface effects {
                go: func() -> s32 {
                    shared let counter = 0;
                    atomic { counter += 1; };
                    let v = atomic { counter };
                    v;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");

        let options = CompileOptions { threads: true, ..Default::default() };
        let wasm = compile_module(&ast, &options).expect("atomic block desugaring should compile");
        assert!(wasm.starts_with(&[0x00, 0x61, 0x73, 0x6d]));
    }

    #[test]
    fn test_compile_await_thread_id() {
        let source = r#"
            package local:test;

            interface effects {
                go: func() -> s32 {
                    let tid = spawn { 1; };
                    await tid;
                    0;
                }
            }
        "#;

        let (ast, _) = parse_file(source);
        let ast = ast.expect("Should parse");

        let options = CompileOptions { threads: true, ..Default::default() };
        let wasm = compile_module(&ast, &options).expect("await thread-id should compile");
        assert!(wasm.starts_with(&[0x00, 0x61, 0x73, 0x6d]));
    }
}
