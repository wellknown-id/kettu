# Kettu

Kettu is a WASM-first programming language with native WebAssembly Component Model support. Any valid `.wit` file is a valid `.kettu` file — Kettu extends WIT with function bodies, control flow, and concurrency primitives.

## Features

- **WIT compatible** — pure WIT files parse and compile unchanged
- **Function bodies** — implement logic directly in interface declarations
- **Concurrency** — `spawn`, `await`, `shared let`, `atomic { }` blocks
- **WASM threads** — shared memory, atomics, thread-spawn via `--threads`
- **Async/await** — WASI Preview 3 async ABI via `--wasip3`
- **Component Model** — compiles to standard WASM components
- **Integrated LSP** — hover, completion, diagnostics, go-to-definition

## Example

```kettu
package local:demo;

interface counter {
    run: func() -> s32 {
        shared let total = 0;

        let t1 = spawn { atomic { total += 100; }; };
        let t2 = spawn { atomic { total += 200; }; };

        await t1;
        await t2;

        atomic { total };
    }
}
```

## Quick start

```bash
cargo build --workspace
cargo test --workspace

# Build a Kettu file
kettu build example.kettu
kettu build --threads example.kettu   # with concurrency
```

## Documentation

- [Language Overview](docs/language-overview.md) — types, control flow, functions
- [Concurrency](docs/concurrency.md) — threads, atomics, shared memory

## Toolchain

| Crate | Purpose |
|-------|---------|
| `kettu-parser` | Parser, grammar, AST |
| `kettu-checker` | Semantic analysis, type checking |
| `kettu-codegen` | WebAssembly code generation |
| `kettu-lsp` | Language Server Protocol |
| `kettu-cli` | CLI binary (`kettu`) |
| `kettu-wasm` | WASM-packaged compiler |

## Editor tooling

The VS Code extension lives under `crates/kettu-cli/editors/vscode/`.

## License

Apache-2.0
