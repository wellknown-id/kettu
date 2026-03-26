//! Kettu CLI
//!
//! Command-line interface for the Kettu compiler.

use clap::{Parser, Subcommand};
use std::fs;
use std::path::PathBuf;

fn load_imported_asts(
    file: &PathBuf,
    ast: &kettu_parser::WitFile,
) -> Vec<(String, kettu_parser::WitFile)> {
    let resolved = kettu_codegen::resolve_imports(file, ast);
    let mut imported_asts: Vec<(String, kettu_parser::WitFile)> = Vec::new();

    for (alias, (import_path, _interface_name)) in &resolved.imports {
        if import_path.exists() {
            match fs::read_to_string(import_path) {
                Ok(content) => {
                    let (imported_ast, import_errors) = kettu_parser::parse_file(&content);
                    if !import_errors.is_empty() {
                        for error in &import_errors {
                            eprintln!("Parse error in {}: {}", import_path.display(), error);
                        }
                        std::process::exit(1);
                    }
                    if let Some(ast) = imported_ast {
                        imported_asts.push((alias.clone(), ast));
                    }
                }
                Err(e) => {
                    eprintln!(
                        "Warning: Could not load import {}: {}",
                        import_path.display(),
                        e
                    );
                }
            }
        }
    }

    imported_asts
}

#[derive(Parser)]
#[command(name = "kettu")]
#[command(about = "Kettu - A WASM-first programming language", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Parse a .kettu or .wit file and print the AST
    Parse {
        /// Input file
        file: PathBuf,
    },
    /// Type-check a .kettu or .wit file
    Check {
        /// Input file
        file: PathBuf,
    },
    /// Build a WASM component from a .kettu file
    Build {
        /// Input file
        file: PathBuf,
        /// Output file (defaults to input with .wasm extension)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Build only a core module (no component wrapping)
        #[arg(long)]
        core: bool,
        /// Enable WASI Preview 3 async ABI (experimental)
        #[arg(long)]
        wasip3: bool,
    },
    /// Run tests in a .kettu file
    Test {
        /// Input file or directory
        file: PathBuf,
        /// Filter tests by name
        #[arg(long)]
        filter: Option<String>,
    },
    /// Start the LSP server (stdio)
    Lsp {
        /// Use stdio transport (default, accepted for VS Code compatibility)
        #[arg(long)]
        stdio: bool,
    },
    /// Emit pure WIT (strip Kettu extensions like function bodies)
    EmitWit {
        /// Input file
        file: PathBuf,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Parse { file } => {
            let content = match fs::read_to_string(&file) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error reading file: {}", e);
                    std::process::exit(1);
                }
            };

            let (ast, errors) = kettu_parser::parse_file(&content);

            if !errors.is_empty() {
                for error in &errors {
                    eprintln!("Parse error: {}", error);
                }
            }

            if let Some(ast) = ast {
                println!("{:#?}", ast);
            } else {
                eprintln!("Failed to parse file");
                std::process::exit(1);
            }
        }

        Commands::Check { file } => {
            let content = match fs::read_to_string(&file) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error reading file: {}", e);
                    std::process::exit(1);
                }
            };

            let (ast, parse_errors) = kettu_parser::parse_file(&content);

            if !parse_errors.is_empty() {
                for error in &parse_errors {
                    eprintln!("Parse error: {}", error);
                }
                std::process::exit(1);
            }

            if let Some(ast) = ast {
                let imported_asts = load_imported_asts(&file, &ast);
                let mut files_to_check: Vec<kettu_parser::WitFile> = imported_asts
                    .iter()
                    .map(|(_, imported)| imported.clone())
                    .collect();

                files_to_check.push(ast);
                let diagnostics = if files_to_check.len() == 1 {
                    kettu_checker::check(&files_to_check[0])
                } else {
                    kettu_checker::check_package(&files_to_check)
                };

                if diagnostics.is_empty() {
                    println!("✓ No errors found");
                } else {
                    for diag in &diagnostics {
                        let prefix = match diag.severity {
                            kettu_checker::Severity::Error => "error",
                            kettu_checker::Severity::Warning => "warning",
                            kettu_checker::Severity::Info => "info",
                        };
                        eprintln!("{}: {}", prefix, diag.message);
                    }
                    std::process::exit(1);
                }
            }
        }

        Commands::Build {
            file,
            output,
            core,
            wasip3,
        } => {
            let content = match fs::read_to_string(&file) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error reading file: {}", e);
                    std::process::exit(1);
                }
            };

            let (ast, parse_errors) = kettu_parser::parse_file(&content);

            if !parse_errors.is_empty() {
                for error in &parse_errors {
                    eprintln!("Parse error: {}", error);
                }
                std::process::exit(1);
            }

            let ast = match ast {
                Some(a) => a,
                None => {
                    eprintln!("Failed to parse file");
                    std::process::exit(1);
                }
            };

            // Resolve and load imports FIRST (before type checking)
            let imported_asts = load_imported_asts(&file, &ast);
            let imported_aliases: std::collections::HashSet<String> = imported_asts
                .iter()
                .map(|(alias, _)| alias.clone())
                .collect();

            // Type check (filtering out errors for imported interface references)
            let diagnostics = kettu_checker::check(&ast);
            let errors: Vec<_> = diagnostics
                .iter()
                .filter(|d| matches!(d.severity, kettu_checker::Severity::Error))
                .filter(|d| {
                    // Skip "Unknown variable" errors for imported interface aliases
                    if d.message.starts_with("Unknown variable: ") {
                        let var_name = d.message.trim_start_matches("Unknown variable: ");
                        !imported_aliases.contains(var_name)
                    } else {
                        true
                    }
                })
                .collect();

            if !errors.is_empty() {
                for diag in &errors {
                    eprintln!("error: {}", diag.message);
                }
                std::process::exit(1);
            }

            // Compile
            let compile_options = kettu_codegen::CompileOptions {
                core_only: core,
                memory_pages: 1,
                wasip3,
            };

            let wasm = if core {
                if imported_asts.is_empty() {
                    kettu_codegen::build_core_module(&ast, &compile_options)
                } else {
                    let imports_refs: Vec<_> = imported_asts
                        .iter()
                        .map(|(alias, ast)| (alias.clone(), ast))
                        .collect();
                    kettu_codegen::compile_module_with_imports(
                        &ast,
                        &imports_refs,
                        &compile_options,
                    )
                }
            } else {
                let component_options = kettu_codegen::ComponentOptions {
                    compile: compile_options,
                    bundle_modules: vec![],
                };
                kettu_codegen::build_component(&ast, &component_options)
            };

            match wasm {
                Ok(bytes) => {
                    let output_path = output.unwrap_or_else(|| file.with_extension("wasm"));

                    if let Err(e) = fs::write(&output_path, &bytes) {
                        eprintln!("Error writing output: {}", e);
                        std::process::exit(1);
                    }

                    println!("✓ Built {} ({} bytes)", output_path.display(), bytes.len());
                }
                Err(e) => {
                    eprintln!("Compile error: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Lsp { .. } => {
            kettu_lsp::run_server().await;
        }

        Commands::EmitWit { file } => {
            let content = match fs::read_to_string(&file) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error reading file: {}", e);
                    std::process::exit(1);
                }
            };

            let (ast, errors) = kettu_parser::parse_file(&content);

            if !errors.is_empty() {
                for error in &errors {
                    eprintln!("Parse error: {}", error);
                }
                std::process::exit(1);
            }

            if let Some(ast) = ast {
                let wit_output = kettu_parser::emit_wit(&ast);
                print!("{}", wit_output);
            }
        }

        Commands::Test { file, filter } => {
            if file.is_dir() {
                // Recursively find all .kettu files
                use walkdir::WalkDir;
                let mut total_passed = 0;
                let mut total_failed = 0;
                let mut files_tested = 0;

                for entry in WalkDir::new(&file)
                    .into_iter()
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().map_or(false, |ext| ext == "kettu"))
                {
                    let path = entry.path().to_path_buf();
                    let (passed, failed) = run_tests(&path, filter.as_deref());
                    total_passed += passed;
                    total_failed += failed;
                    files_tested += 1;
                }

                if files_tested == 0 {
                    eprintln!("No .kettu files found in {}", file.display());
                    std::process::exit(1);
                }

                println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!(
                    "Total: {} passed, {} failed across {} file(s)",
                    total_passed, total_failed, files_tested
                );
                if total_failed > 0 {
                    std::process::exit(1);
                }
            } else {
                let (passed, failed) = run_tests(&file, filter.as_deref());
                if failed > 0 {
                    std::process::exit(1);
                }
                let _ = (passed, failed); // Suppress unused warning
            }
        }
    }
}

/// Convert a byte offset to a 1-based line number.
fn offset_to_line(source: &str, offset: usize) -> usize {
    source[..offset.min(source.len())]
        .bytes()
        .filter(|&b| b == b'\n')
        .count()
        + 1
}

/// Run tests in a Kettu file, returns (passed, failed) counts
fn run_tests(file: &PathBuf, filter: Option<&str>) -> (usize, usize) {
    use kettu_parser::{Gate, InterfaceItem, TopLevelItem};
    use std::time::Instant;
    use wasmtime::{Engine, Instance, Module, Store};

    let content = match fs::read_to_string(file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error reading file: {}", e);
            std::process::exit(1);
        }
    };

    let (ast, parse_errors) = kettu_parser::parse_file(&content);

    if !parse_errors.is_empty() {
        for error in &parse_errors {
            eprintln!("Parse error: {}", error);
        }
        std::process::exit(1);
    }

    let ast = match ast {
        Some(a) => a,
        None => {
            eprintln!("Failed to parse file");
            std::process::exit(1);
        }
    };

    // Type check
    let diagnostics = kettu_checker::check(&ast);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| matches!(d.severity, kettu_checker::Severity::Error))
        .collect();

    if !errors.is_empty() {
        for diag in &errors {
            eprintln!("error: {}", diag.message);
        }
        std::process::exit(1);
    }

    // Discover test functions — collect name, func, and source line number
    let mut tests: Vec<(&str, &kettu_parser::Func, usize)> = Vec::new();

    for item in &ast.items {
        if let TopLevelItem::Interface(iface) = item {
            for iface_item in &iface.items {
                if let InterfaceItem::Func(func) = iface_item {
                    // Check if function has @test gate
                    let is_test = func.gates.iter().any(|g| matches!(g, Gate::Test));
                    if is_test {
                        let name = &func.name.name;
                        // Apply filter if specified
                        if let Some(f) = filter {
                            if !name.contains(f) {
                                continue;
                            }
                        }
                        let line = offset_to_line(&content, func.span.start);
                        tests.push((name, func, line));
                    }
                }
            }
        }
    }

    if tests.is_empty() {
        println!("No tests found in {}", file.display());
        return (0, 0);
    }

    // Compile the module once
    let compile_options = kettu_codegen::CompileOptions {
        core_only: true,
        memory_pages: 1,
        wasip3: false,
    };

    let wasm_bytes = match kettu_codegen::build_core_module(&ast, &compile_options) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("Compile error: {}", e);
            std::process::exit(1);
        }
    };

    // Create wasmtime engine and module
    let engine = Engine::default();
    let module = match Module::new(&engine, &wasm_bytes) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Failed to load WASM module: {:?}", e);
            std::process::exit(1);
        }
    };

    println!("Running {} test(s) in {}...\n", tests.len(), file.display());

    let mut passed = 0;
    let mut failed = 0;

    let file_display = file.display();

    for (name, func, line) in &tests {
        let start = Instant::now();

        // Check if test function has a body and returns bool
        let has_body = func.body.is_some();
        let returns_bool = func
            .result
            .as_ref()
            .map(|ty| {
                matches!(
                    ty,
                    kettu_parser::Ty::Primitive(kettu_parser::PrimitiveTy::Bool, _)
                )
            })
            .unwrap_or(false);

        if !has_body {
            println!("  ✗ {} (no body) — {}:{}", name, file_display, line);
            failed += 1;
            continue;
        }

        if !returns_bool {
            println!("  ✗ {} (must return bool) — {}:{}", name, file_display, line);
            failed += 1;
            continue;
        }

        // Create a fresh store for each test
        let mut store = Store::new(&engine, ());

        // Instantiate module
        let instance = match Instance::new(&mut store, &module, &[]) {
            Ok(i) => i,
            Err(e) => {
                println!("  ✗ {} (instantiation failed: {}) — {}:{}", name, e, file_display, line);
                failed += 1;
                continue;
            }
        };

        // Get the test function — exports may use qualified names (ns:pkg/iface#func)
        // so search for an export ending with #name, falling back to bare name
        let export_name = {
            let mut found = None;
            for export in instance.exports(&mut store) {
                let ename = export.name();
                if ename == *name || ename.ends_with(&format!("#{}", name)) {
                    found = Some(ename.to_string());
                    break;
                }
            }
            found
        };

        let export_name = match export_name {
            Some(n) => n,
            None => {
                let elapsed = start.elapsed();
                let func_exports: Vec<String> = instance
                    .exports(&mut store)
                    .map(|e| e.name().to_string())
                    .filter(|n| n != "memory" && n != "cabi_realloc" && n != "cabi_arena_reset")
                    .collect();
                println!("  ✗ {} (not found in exports) ({:.1?}) — {}:{}", name, elapsed, file_display, line);
                if !func_exports.is_empty() {
                    println!("    available: {}", func_exports.join(", "));
                }
                failed += 1;
                continue;
            }
        };

        let test_func = match instance.get_typed_func::<(), i32>(&mut store, &export_name) {
            Ok(f) => f,
            Err(e) => {
                let elapsed = start.elapsed();
                println!("  ✗ {} (signature mismatch: {}) ({:.1?}) — {}:{}", name, e, elapsed, file_display, line);
                failed += 1;
                continue;
            }
        };

        // Execute the test
        match test_func.call(&mut store, ()) {
            Ok(result) => {
                let elapsed = start.elapsed();
                if result != 0 {
                    println!("  ✓ {} ({:.1?})", name, elapsed);
                    passed += 1;
                } else {
                    println!("  ✗ {} (returned false) ({:.1?}) — {}:{}", name, elapsed, file_display, line);
                    failed += 1;
                }
            }
            Err(e) => {
                let elapsed = start.elapsed();
                println!("  ✗ {} (execution error: {}) ({:.1?}) — {}:{}", name, e, elapsed, file_display, line);
                failed += 1;
            }
        }
    }

    println!();
    if failed > 0 {
        println!("Results: {} passed, {} failed", passed, failed);
    } else {
        println!("Results: {} passed", passed);
    }
    (passed, failed)
}
