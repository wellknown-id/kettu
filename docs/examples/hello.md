---
// docs-meta: controls how this page appears in `kettu docs`
// section: "Examples"
// order: 1
// title: "Hello World"
// file: "hello-world"
// keywords: "hello, world, basics, simple, greetings"
---

# Hello World Example

This example demonstrates the core building blocks of a Kettu program: packages, interfaces, and functions.

## The Code

```kettu
package example:hello;

/// A simple greeting interface
interface greetings {
    /// Get a greeting message
    get-greeting: func() -> string {
        "Hello, World!"
    }

    /// Get a farewell message
    get-farewell: func() -> string {
        "Goodbye!"
    }

    /// Echo a number back
    echo: func(n: s32) -> s32 {
        n
    }
}

/// A world that exports our greeting functionality
world hello-world {
    export greetings;
}
```

## Explanation

1.  **Package**: `package example:hello;` defines the namespace for this component.
2.  **Interface**: The `interface greetings` block defines a set of related functions.
3.  **Functions**: Each function specifies its parameters and return type. In Kettu, the last expression in a block is the return value.
4.  **World**: The `world hello-world` block describes the final component. By exporting the `greetings` interface, we make it available to other components or the host environment.
