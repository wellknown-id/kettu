# Kettu

**Kettu** is a WASM-first programming language with native WASM component model support.

## Features

- **WIT Compatible**: Any `.wit` file is a valid `.kettu` file
- **Extended Syntax**: Function bodies in interface declarations (Kettu extension)
- **WASM Native**: Compiler itself runs as a WASM module
- **Integrated LSP**: Language server built into the compiler

## Example

Standard WIT:
```wit
package local:demo;

interface host {
    log: func(msg: string);
}
```

Kettu extension (function bodies):
```kettu
package local:demo;

interface host {
    log: func(msg: string) {
        println(msg)
    }
}
```

## Building

```bash
cargo build --release
```

## Usage

```bash
# Parse a file and print AST
kettu parse file.kettu

# Type-check a file
kettu check file.kettu

# Start LSP server
kettu lsp

# Start Debug Adapter Protocol server (stdio)
kettu dap

# Emit pure WIT (strip function bodies)
kettu emit-wit file.kettu
```

## Project Structure

```
crates/
├── kettu-parser/    # Lexer, Parser (chumsky), AST
├── kettu-checker/   # Type checking, validation
├── kettu-codegen/   # WebAssembly code generation
├── kettu-lsp/       # Language Server Protocol
├── kettu-cli/       # Command-line interface
└── kettu-wasm/      # WASM-packaged compiler pipeline
```

## Contributor Notes

- LSP hover behavior and inference scope: [../kettu-lsp/HOVER-CAPABILITIES.md](../kettu-lsp/HOVER-CAPABILITIES.md)

## License

Apache-2.0
