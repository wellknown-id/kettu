//! Kettu Codegen
//!
//! WASM code generation for Kettu function bodies.
//! Produces either core WASM modules or Component Model assemblies.

mod compiler;
mod component;
mod resolver;

pub use compiler::{CompileError, CompileOptions, compile_module, compile_module_with_imports};
pub use component::{ComponentOptions, build_component, build_core_module};
pub use resolver::{ResolvedImports, resolve_imports};
