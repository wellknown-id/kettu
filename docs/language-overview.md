# Kettu Language Overview

Kettu is a WASM-first programming language. Any valid `.wit` file is a valid `.kettu` file — Kettu extends WIT with function bodies, control flow, and concurrency primitives.

## Basics

```kettu
package local:demo;

interface greeter {
    greet: func(name: string) -> string {
        "Hello, " + name + "!";
    }
}
```

### Types

| Kettu          | WIT            | WASM                   |
| -------------- | -------------- | ---------------------- |
| `s32`          | `s32`          | `i32`                  |
| `u32`          | `u32`          | `i32`                  |
| `s64`          | `s64`          | `i64`                  |
| `bool`         | `bool`         | `i32`                  |
| `string`       | `string`       | `i32` (ptr+len)        |
| `option<T>`    | `option<T>`    | discriminant + payload |
| `result<T, E>` | `result<T, E>` | discriminant + payload |

### Variables and Assignments

```kettu
let x = 42;
let name = "kettu";
x = 100;        // reassignment
x += 5;         // compound add
x -= 2;         // compound subtract
```

### Control Flow

```kettu
// Conditionals
if x > 0 { x; } else { 0; };

// While loops
let i = 0;
while i < 10 {
    i += 1;
};

// For-each loops
for item in list {
    process(item);
};

// Pattern matching
match value {
    some(x) => x,
    none => 0,
};
```

### Functions and Closures

```kettu
// Named functions in interfaces
interface math {
    add: func(a: s32, b: s32) -> s32 {
        a + b;
    }
}

// Lambdas
let double = |x| { x * 2; };
```

## Concurrency

See [concurrency.md](concurrency.md) for the full guide.

```kettu
shared let counter = 0;

let tid = spawn {
    atomic { counter += 1; };
};

await tid;
let value = atomic { counter };
```

## Async/Await

Kettu supports WASI Preview 3 async functions with `--wasip3`:

```kettu
interface api {
    fetch: async func(url: string) -> string {
        let result = await http.get(url);
        result;
    }
}
```

## Building

```bash
kettu build file.kettu              # → component .wasm
kettu build --threads file.kettu    # → with shared memory
kettu build --core-only file.kettu  # → core module only
kettu check file.kettu              # type-check only
kettu test .                        # run all tests
```
