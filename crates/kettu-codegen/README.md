# kettu-codegen

Code generator for the Kettu language.

The `kettu-codegen` crate translates the verified Kettu AST into high-performance WebAssembly modules.

## Key Features

- **WebAssembly Generation**: Direct emission of WASM bytecode using the `wasm-encoder` crate.
- **Component Model Support**: First-class support for producing WebAssembly Components, including WIT interface generation and linking.
- **Dependency Resolution**: Implements logic to resolve and link external WIT interfaces at compile time.
- **Optimization**: Performs basic code-level optimizations during the emission process.
- **Multi-Module Layout**: Supports organizing generated code into multiple logical modules as part of a single WASM Component.

## Components

- `compiler.rs`: The main code emission logic for WASM.
- `component.rs`: Logic for wrapping core modules into Components.
- `resolver.rs`: WIT dependency resolution for Kettu.
