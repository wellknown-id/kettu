//! Component Builder
//!
//! Wraps core WASM modules into Component Model assemblies.
//! Uses wit-component for proper component model encoding.

use crate::compiler::{CompileError, CompileOptions, compile_module};
use kettu_parser::{WitFile, emit_wit};

/// Options for building a component
#[derive(Debug, Clone, Default)]
pub struct ComponentOptions {
    /// Compile options for the core module
    pub compile: CompileOptions,
    /// Additional modules to bundle (paths to .wasm files)
    pub bundle_modules: Vec<std::path::PathBuf>,
}

/// Build a WASM Component from a Kettu file
///
/// This produces a Component Model assembly that:
/// - Contains the compiled Kettu code as a core module
/// - Includes the WIT interface for type information
/// - Optionally bundles additional modules
pub fn build_component(
    file: &WitFile,
    options: &ComponentOptions,
) -> Result<Vec<u8>, CompileError> {
    // First, compile to a core module
    let core_wasm = compile_module(file, &options.compile)?;

    // Generate the WIT interface
    let wit = emit_wit(file);

    // Use wit-component to create the component
    // This wraps the core module with Component Model metadata
    let component = wrap_component(&core_wasm, &wit, options)?;

    Ok(component)
}

/// Wrap a core module into a component
fn wrap_component(
    core_wasm: &[u8],
    wit: &str,
    _options: &ComponentOptions,
) -> Result<Vec<u8>, CompileError> {
    use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};
    use wit_parser::{Resolve, UnresolvedPackageGroup};

    // Parse the WIT string into a Resolve
    let mut resolve = Resolve::default();

    // For wasip3, inject the canon-async interface stub that contains
    // the async primitives the runtime will provide
    let wit_with_async = if _options.compile.wasip3 {
        // Inject canon-async interface at the end of the WIT
        // The world needs to import it for wit-component to resolve
        let async_stub = r#"
interface canon-async {
    task-return: func(val: s32);
    waitable-set-new: func() -> s32;
    waitable-set-wait: func(ws: s32, out-ptr: s32) -> s32;
    subtask-drop: func(subtask: s32);
}
"#;
        // Insert 'import canon-async;' into the world definition
        let modified_wit = wit.replace("export ", "import canon-async;\n    export ");
        format!("{}\n{}", modified_wit, async_stub)
    } else {
        wit.to_string()
    };

    let pkg_group =
        UnresolvedPackageGroup::parse("component.wit", &wit_with_async).map_err(|e| {
            CompileError {
                message: format!("Failed to parse WIT: {}", e),
                span: None,
            }
        })?;

    let package_id = resolve.push_group(pkg_group).map_err(|e| CompileError {
        message: format!("Failed to resolve WIT package: {}", e),
        span: None,
    })?;

    // Find the world in the package
    let world_id = resolve.packages[package_id]
        .worlds
        .values()
        .next()
        .copied()
        .ok_or_else(|| CompileError {
            message: "No world found in WIT package".to_string(),
            span: None,
        })?;

    // Embed WIT metadata into the core module
    let mut module_with_metadata = core_wasm.to_vec();
    embed_component_metadata(
        &mut module_with_metadata,
        &resolve,
        world_id,
        StringEncoding::UTF8,
    )
    .map_err(|e| CompileError {
        message: format!("Failed to embed component metadata: {}", e),
        span: None,
    })?;

    // Now encode as component
    let mut encoder = ComponentEncoder::default()
        .validate(true)
        .module(&module_with_metadata)
        .map_err(|e| CompileError {
            message: format!("Failed to encode core module: {}", e),
            span: None,
        })?;

    let component_bytes = encoder.encode().map_err(|e| {
        // Print full error chain for debugging
        let mut error_chain = format!("{}", e);
        let mut source = e.source();
        while let Some(cause) = source {
            error_chain.push_str(&format!("\n  caused by: {}", cause));
            source = cause.source();
        }
        CompileError {
            message: format!("Failed to build component: {}", error_chain),
            span: None,
        }
    })?;

    Ok(component_bytes)
}

/// Build a core module only (no component wrapping)
pub fn build_core_module(
    file: &WitFile,
    options: &CompileOptions,
) -> Result<Vec<u8>, CompileError> {
    compile_module(file, options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kettu_parser::parse_file;

    #[test]
    fn test_build_core_module() {
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
        let wasm = build_core_module(&ast, &options).expect("Should build");

        // Verify WASM magic number
        assert_eq!(&wasm[0..4], b"\0asm");
    }
}
