# kettu-wasm

WebAssembly-targeted library for the Kettu compiler.

The `kettu-wasm` crate exposes the core Kettu compilation pipeline (parser, checker, and codegen) as a WebAssembly module.

## Key Features

- **Browser-Native Compilation**: Enables compiling Kettu source code directly in the browser without a backend server.
- **Playground Support**: Specifically optimized for use in the Kettu interactive playground and developer tools.
- **Library API**: Provides a high-level WASM/JS API for parsing and validating Kettu code from WebAssembly hosts.
- **Integrated Toolchain**: Bundles the necessary parts of `kettu-parser` and `kettu-checker` into a compact, specialized binary.

## Structure

- `lib.rs`: The entry point for the WASM library, containing the public JS-to-WASM bindings.
