# Kettu Examples

This directory contains `.kettu` files demonstrating the features of the Kettu language. Run `cargo run -p kettu-cli test examples/` from the repo root to execute all tests.

## Examples

### Core Language

| File | Description |
| --- | --- |
| [hello.kettu](hello.kettu) | Packages, interfaces, imports |
| [math.kettu](math.kettu) | Arithmetic operators, local variables, return statements |
| [math_test.kettu](math_test.kettu) | Unit tests with `@test` annotations |
| [control.kettu](control.kettu) | If/else expressions, comparisons |
| [control_test.kettu](control_test.kettu) | Control flow tests |
| [types.kettu](types.kettu) | Records, enums, type aliases |
| [assignment_test.kettu](assignment_test.kettu) | Reassignment and compound assignment (`+=`, `-=`) |
| [negation_test.kettu](negation_test.kettu) | Unary negation (`-expr`, `-5`, `-(-x)`) |

### Data Structures

| File | Description |
| --- | --- |
| [record_test.kettu](record_test.kettu) | Record construction and field access |
| [variant_test.kettu](variant_test.kettu) | Variant types, `#case` / `type#case(payload)` construction |
| [match_test.kettu](match_test.kettu) | Pattern matching on variants |
| [list_test.kettu](list_test.kettu) | Lists, indexing, slicing, `list-push`, `list-set` |
| [option_result_test.kettu](option_result_test.kettu) | `some(x)` / `none`, `ok(x)` / `err(e)` |
| [try_test.kettu](try_test.kettu) | Try operator (`?`) and optional chaining (`?.`) |

### Strings

| File | Description |
| --- | --- |
| [string_test.kettu](string_test.kettu) | String concatenation, `str-len`, `str-eq` |
| [string_interp_test.kettu](string_interp_test.kettu) | String interpolation (`"hello {name}"`) |

### Loops

| File | Description |
| --- | --- |
| [loop_test.kettu](loop_test.kettu) | `for x in range(...)` loops |
| [while_test.kettu](while_test.kettu) | `while` loops |
| [if_expr_test.kettu](if_expr_test.kettu) | If-as-expression, `break`, `continue` in loops |

### Functions & Closures

| File | Description |
| --- | --- |
| [hof_test.kettu](hof_test.kettu) | `map`, `filter`, `reduce` with lambdas |
| [callable_closure_test.kettu](callable_closure_test.kettu) | Closures capturing outer variables |
| [trailing_closure_test.kettu](trailing_closure_test.kettu) | Trailing closure syntax (`func \|x\| expr`) |

### Resources & Modules

| File | Description |
| --- | --- |
| [resources.kettu](resources.kettu) | Resource types, constructors, instance/static methods |
| [resource_test.kettu](resource_test.kettu) | Resource tests |
| [versioned.kettu](versioned.kettu) | Versioned package paths (`@version`) |
| [composition.kettu](composition.kettu) | World composition with `include` |
| [modules/](modules/) | Multi-file compilation (`main.kettu` + `helper/lib.kettu`) |

### Concurrency

| File | Description |
| --- | --- |
| [async_test.kettu](async_test.kettu) | Async functions and `await` |
| [async_callback_test.kettu](async_callback_test.kettu) | Async callbacks |
| [thread_test.kettu](thread_test.kettu) | `spawn { }`, `thread.join`, `shared let`, `atomic { }` |

### SIMD

| File | Description |
| --- | --- |
| [simd_test.kettu](simd_test.kettu) | `v128` ops, `simd for`, lane operations |

## Running

```bash
# Run all tests
cargo run -p kettu-cli test examples/

# Run a specific test file
cargo run -p kettu-cli test examples/math_test.kettu

# Parse and display AST
cargo run -p kettu-cli parse examples/hello.kettu

# Type check
cargo run -p kettu-cli check examples/math.kettu

# Emit pure WIT (strips function bodies)
cargo run -p kettu-cli emit-wit examples/hello.kettu

# Compile to WASM
cargo run -p kettu-cli build --core examples/math.kettu -o output.wasm
```

## Language Quick Reference

### Operators
`+`, `-`, `*`, `/`, `==`, `!=`, `<`, `>`, `<=`, `>=`, `&&`, `||`, `!`, `-` (unary)

### Subtraction vs Hyphens
Identifiers use kebab-case (`my-var`, `str-len`). The `-` character is part of the identifier when adjacent to letters: `a-b` is one identifier. **Use spaces for subtraction:** `a - b`.

### Variant Syntax
- Construction: `#case`, `#case(payload)`, `type#case`, `type#case(payload)`
- Pattern matching: `type#case(binding)` in `match` arms
