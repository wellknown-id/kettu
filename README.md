# Kettu

Kettu is a WASM-first programming language with native WebAssembly Component Model support.

This repository contains the full Kettu toolchain:

- `crates/kettu-parser` — parser and AST
- `crates/kettu-checker` — semantic analysis and type checking
- `crates/kettu-codegen` — WebAssembly code generation
- `crates/kettu-lsp` — language-server implementation
- `crates/kettu-cli` — the `kettu` CLI binary
- `crates/kettu-wasm` — WebAssembly-packaged compilation pipeline

## Quick start

```bash
cargo build --workspace
cargo test --workspace
```

## Editor tooling

The VS Code extension sources live under `crates/kettu-cli/editors/vscode/`.
