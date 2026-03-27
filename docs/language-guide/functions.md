---
// docs-meta: controls how this page appears in `kettu docs`
// section: "Language Topics"
// order: 3
// title: "Functions"
// file: "functions"
// preamble-start
//   let a = 1;
//   let b = 2;
//   let x = 10;
//   let y = 20;
// preamble-end
// keywords: "function, func, return, closure, lambda, async, parameter, argument"
---
# Functions

## Function Signatures

Functions are declared with parameters and optional return type:

```kettu
// No parameters, no return
do-something: func();

// Parameters
greet: func(name: string);

// Return type
get-value: func() -> s32;

// Parameters and return
add: func(a: s32, b: s32) -> s32;
```

## Function Bodies (Kettu Extension)

Kettu extends WIT by allowing function implementations:

```kettu
interface math {
    add: func(a: s32, b: s32) -> s32 {
        return a + b;
    }
    
    max: func(a: s32, b: s32) -> s32 {
        if a > b { a } else { b }
    }
}
```

## Statements

### Let Bindings

```kettu
let x = 10;
let result = a + b;
let flag = x > 0;
```

### Return

```kettu
return 42;
return x + y;
return;  // For functions with no return type
```

### Expression Statements

The last expression in a function body is implicitly returned:

```kettu nocheck
max: func(a: s32, b: s32) -> s32 {
    if a > b { a } else { b }  // Implicit return
}
```

## Async Functions

Mark functions as async for WASI Preview 3:

```kettu nocheck
fetch: async func(url: string) -> string;

// With body
process: async func(data: string) -> string {
    let result = await transform(data);
    result;
}
```

See [WASI Preview 3](../wasip3.md) for details on async/await.

## Lambdas & Closures

See [Closures & HOFs](./closures.md) for anonymous functions and higher-order patterns:

```kettu
let double = |x| x * 2;
let arr = map([1, 2, 3], |x| x + 1);
```

