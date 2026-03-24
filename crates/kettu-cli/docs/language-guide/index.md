# Kettu Language Guide

Kettu is a programming language that extends [WIT (WebAssembly Interface Types)](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md) with function bodies, enabling both interface definitions and implementation in a single file.

## Quick Start

```kettu
package example:hello;

interface greeter {
    greet: func(name: string) -> string {
        return "Hello, " + name + "!";
    }
}

world hello-world {
    export greeter;
}
```

## Language Topics

| Topic | Description |
|-------|-------------|
| [Package & Interface](./packages.md) | Declaring packages, interfaces, and worlds |
| [Data Types](./types.md) | Primitives, records, variants, enums, flags |
| [Functions](./functions.md) | Function signatures and bodies |
| [Expressions](./expressions.md) | Operators, literals, and control flow |
| [Resources](./resources.md) | Resource types with methods |
| [Testing](./testing.md) | Built-in test framework |
| [Feature Gates](./gates.md) | @since, @deprecated, @unstable |

## CLI Commands

```bash
kettu build file.kettu     # Compile to WASM component
kettu build --core file.kettu  # Compile to core WASM module
kettu check file.kettu     # Type check only
kettu parse file.kettu     # Show parsed AST
kettu test file.kettu      # Run tests
kettu lsp                  # Start LSP server
```

## Compatibility

Kettu is a **superset of WIT** — any valid `.wit` file is also valid Kettu. The extension adds:
- Function bodies inside interface declarations
- Expression syntax (operators, if/else, let bindings)
- `@test` annotation for unit tests
- `assert` expressions
