---
// docs-meta: controls how this page appears in `kettu docs`
// section: "Examples"
// order: 2
// title: "Mathematical Operations"
// file: "math-operations"
// keywords: "math, calculator, arithmetic, functions, variables"
---

# Mathematical Operations

This example explores Kettu's arithmetic operations, variable bindings with `let`, and function return values.

## The Code

```kettu
package example:math;

/// Mathematical operations interface
interface calculator {
    /// Add two numbers
    add: func(a: s32, b: s32) -> s32 {
        let result = a + b;
        return result;
    }

    /// Subtract two numbers
    subtract: func(a: s32, b: s32) -> s32 {
        return a - b;
    }

    /// Multiply two numbers
    multiply: func(a: s32, b: s32) -> s32 {
        return a * b;
    }

    /// Integer division
    divide: func(a: s32, b: s32) -> s32 {
        return a / b;
    }

    /// Compute average of three numbers
    average: func(x: s32, y: s32, z: s32) -> s32 {
        let sum = x + y;
        let total = sum + z;
        return total / 3;
    }

    /// Check if number is positive
    is-positive: func(n: s32) -> bool {
        return n > 0;
    }
}

world math-world {
    export calculator;
}
```

## Key Concepts

1.  **Variable Bindings**: Use `let` to store intermediate values. Kettu variables are immutable by default.
2.  **Return Statement**: Explicit `return` statements can be used to exit a function early or for clarity, although the last expression is automatically returned.
3.  **Basic Arithmetic**: Kettu supports the standard `+`, `-`, `*`, `/` operators.
4.  **Boolean Logic**: Comparison operators like `>` return a `bool`.
