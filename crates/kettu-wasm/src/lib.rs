//! Kettu WASM Module
//!
//! This crate provides the Kettu compiler as a WASM module.
//! It exports functions for parsing, type-checking, and compiling Kettu code.

use std::alloc::{Layout, alloc, dealloc};

// Re-export core functionality
pub use kettu_checker::check;
pub use kettu_codegen::{
    CompileOptions, ComponentOptions, build_component, build_core_module, compile_module,
};
pub use kettu_parser::{WitFile, emit_wit, parse_file};

/// Allocate memory for WASM host to write into
#[unsafe(no_mangle)]
pub extern "C" fn alloc_bytes(len: usize) -> *mut u8 {
    let layout = Layout::from_size_align(len, 1).unwrap();
    unsafe { alloc(layout) }
}

/// Free allocated memory
#[unsafe(no_mangle)]
pub extern "C" fn free_bytes(ptr: *mut u8, len: usize) {
    let layout = Layout::from_size_align(len, 1).unwrap();
    unsafe { dealloc(ptr, layout) }
}

/// Parse Kettu source code
///
/// Input: pointer to UTF-8 source string
/// Output: 0 on success with AST, 1 on error
#[unsafe(no_mangle)]
pub extern "C" fn kettu_parse(src_ptr: *const u8, src_len: usize) -> i32 {
    let src = unsafe {
        let slice = std::slice::from_raw_parts(src_ptr, src_len);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    let (ast, errors) = parse_file(src);

    if ast.is_some() && errors.is_empty() {
        0
    } else {
        1
    }
}

/// Compile Kettu source to WASM core module
///
/// Returns length of output, writes to output_ptr
#[unsafe(no_mangle)]
pub extern "C" fn kettu_compile_core(
    src_ptr: *const u8,
    src_len: usize,
    output_ptr: *mut *mut u8,
) -> i32 {
    let src = unsafe {
        let slice = std::slice::from_raw_parts(src_ptr, src_len);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    let (ast, errors) = parse_file(src);

    if !errors.is_empty() {
        return -2;
    }

    let ast = match ast {
        Some(a) => a,
        None => return -3,
    };

    let options = CompileOptions::default();
    let wasm = match build_core_module(&ast, &options) {
        Ok(w) => w,
        Err(_) => return -4,
    };

    let len = wasm.len();
    let ptr = alloc_bytes(len);
    unsafe {
        std::ptr::copy_nonoverlapping(wasm.as_ptr(), ptr, len);
        *output_ptr = ptr;
    }

    len as i32
}

/// Emit WIT from Kettu source
///
/// Returns length of output, writes to output_ptr
#[unsafe(no_mangle)]
pub extern "C" fn kettu_emit_wit(
    src_ptr: *const u8,
    src_len: usize,
    output_ptr: *mut *mut u8,
) -> i32 {
    let src = unsafe {
        let slice = std::slice::from_raw_parts(src_ptr, src_len);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    let (ast, errors) = parse_file(src);

    if !errors.is_empty() {
        return -2;
    }

    let ast = match ast {
        Some(a) => a,
        None => return -3,
    };

    let wit = emit_wit(&ast);
    let bytes = wit.into_bytes();
    let len = bytes.len();
    let ptr = alloc_bytes(len);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, len);
        *output_ptr = ptr;
    }

    len as i32
}
