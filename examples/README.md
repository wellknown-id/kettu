# Kettu Examples

This directory contains example `.kettu` files demonstrating various features of the Kettu language.

## Examples

| File                                   | Description                                                            |
| -------------------------------------- | ---------------------------------------------------------------------- |
| [hello.kettu](hello.kettu)             | Basic hello world - packages, interfaces, imports                      |
| [math.kettu](math.kettu)               | Arithmetic operators, local variables, return statements               |
| [control.kettu](control.kettu)         | If/else expressions, comparisons                                       |
| [types.kettu](types.kettu)             | Records, enums, type aliases                                           |
| [resources.kettu](resources.kettu)     | Interface with resource-like operations                                |
| [versioned.kettu](versioned.kettu)     | Versioned package paths (`package/use/import/include` with `@version`) |
| [composition.kettu](composition.kettu) | World composition with include                                         |
| [math_test.kettu](math_test.kettu)     | Unit tests with @test annotations                                      |

## Running Examples

### Parse and display AST

```bash
cargo run -p kettu-cli -- parse examples/hello.kettu
```

### Type check

```bash
cargo run -p kettu-cli -- check examples/math.kettu
```

### Emit pure WIT (strips function bodies)

```bash
cargo run -p kettu-cli -- emit-wit examples/hello.kettu
```

### Compile to WASM

```bash
cargo run -p kettu-cli -- build --core examples/math.kettu -o output.wasm
```

### Run tests

```bash
cargo run -p kettu-cli -- test examples/math_test.kettu

# Filter tests by name
cargo run -p kettu-cli -- test examples/math_test.kettu --filter addition
```

### Validate all examples recursively

```bash
cd kettu
find examples -name '*.kettu' -type f | sort | while read -r f; do
    /mnt/faststorage/repos/kodus/target/debug/kettu check "$f" || exit 1
done
```

## Kettu Language Overview

Kettu is a WASM-first language that extends WIT (WebAssembly Interface Types) with executable function bodies:

### WIT Compatibility

- Full WIT syntax support (packages, interfaces, worlds, types)
- Package-qualified and versioned paths (`pkg:ns/name@version`)
- World composition with `include`

### Kettu Extensions

- Function bodies with expressions
- Local variables (`let x = expr;`)
- Return statements (`return expr;`)
- Binary operators (`+`, `-`, `*`, `/`, `==`, `!=`, `<`, `>`, `<=`, `>=`)
- If/else expressions (`if cond { expr } else { expr }`)
- Function calls
- Test functions with `@test` annotation

### Example Test Function

```kettu
interface math-tests {
    @test
    test-addition: func() -> bool {
        return 2 + 3 == 5;
    }

    @test
    test-let-binding: func() -> bool {
        let a = 10;
        let b = 20;
        return a + b == 30;
    }
}
```

## Current Limitations

The examples in this folder are kept parse/check clean with the current parser/checker.

Known rough edges to keep in mind:

- Feature-gate coverage in examples is intentionally light (most examples focus on executable bodies and type-checking paths).
- Some examples are semantics-oriented (parser/check demos) and do not imply full runtime/resource backing behavior.

### Quick Syntax Notes (Current Parser Behavior)

| Topic                            | Works Reliably in Examples               | Notes                                                                                  |
| -------------------------------- | ---------------------------------------- | -------------------------------------------------------------------------------------- |
| Versioned package paths          | `package/use/import/include ...@version` | See `versioned.kettu`                                                                  |
| Variant construction             | `#case`, `#case(payload)`                | Preferred form in examples                                                             |
| Qualified variant construction   | `type#case`, `type#case(payload)`        | Supported for constructors and `match` patterns (see `variant_test.kettu`)             |
| Qualified pattern payload checks | `type#case(binding)` for payload cases   | Checker enforces arity: payload cases require binding; non-payload cases must not bind |
