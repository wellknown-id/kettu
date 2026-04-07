---
// docs-meta: controls how this page appears in `kettu docs`
// index: true
// file: "index"
// section: "Examples"
// order: 0
// title: "Examples Overview"
---

# Kettu Examples

Learn Kettu by exploring functional code examples. These examples demonstrate the language's syntax and features in a concise, readable format.

## Example Gallery

| Example | Description |
| ------- | ----------- |
| [Hello World](./hello.md) | The quintessential first step: packages, interfaces, and functions. |
| [Mathematical Operations](./math.md) | Arithmetic, variable bindings, and function returns. |
| [Counter Resource](./resources.md) | Managing stateful operations and resource handles. |

## Running the Examples

You can find the source code for these and many other examples in the [examples/](https://github.com/google/kettu/tree/main/examples) directory of the repository.

To run an example using the Kettu CLI:

```bash
# Type-check an example
kettu check examples/hello.kettu

# Compile an example to a WebAssembly Component
kettu build examples/hello.kettu

# Run tests if the example has them
kettu test examples/hello_test.kettu
```

## More Resources

- [Language Guide](../language-guide/index.md): Deep dive into Kettu's syntax and features.
- [Standard Library Documentation](../language-guide/strings.md): Built-in functions and types.
- [Advanced Topics](../concurrency.md): Concurrency, SIMD, and WASI integration.
