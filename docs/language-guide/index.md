---
// docs-meta: controls how this page appears in `kettu docs`
// index: true
// file: "index"
---
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
| [Data Types](./types.md) | Primitives, records, variants, enums, flags, v128 |
| [Functions](./functions.md) | Function signatures, bodies, and async |
| [Expressions](./expressions.md) | Operators, literals, field access, and control flow |
| [Loops & Iteration](./loops.md) | While, for-range, for-each, break/continue |
| [Pattern Matching](./match.md) | Match expressions with variant arms |
| [Lists & Collections](./lists.md) | Literals, indexing, slicing, built-in functions |
| [Closures & HOFs](./closures.md) | Lambdas, captures, map/filter/reduce |
| [Strings](./strings.md) | Concatenation, interpolation, built-in functions |
| [Resources](./resources.md) | Resource types with methods |
| [Testing](./testing.md) | Built-in test framework |
| [Feature Gates](./gates.md) | @since, @deprecated, @unstable |

## Advanced Topics

| Topic | Description |
|-------|-------------|
| [SIMD](../simd.md) | v128 vector operations and `simd for` loops |
| [Concurrency](../concurrency.md) | Threads, atomics, spawn, shared memory |
| [Async/Await](../wasip3.md) | WASI Preview 3 async functions |
| [LSP](../lsp.md) | Language server protocol support |

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
- Expression syntax (operators, if/else, loops, match, let bindings)
- Lists, closures, higher-order functions (`map`, `filter`, `reduce`)
- String interpolation and built-in functions
- SIMD vector operations and vectorized loops
- Concurrency primitives (threads, atomics, spawn)
- `@test` annotation for unit tests

