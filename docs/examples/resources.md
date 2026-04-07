---
// docs-meta: controls how this page appears in `kettu docs`
// section: "Examples"
// order: 3
// title: "Counter Resource"
// file: "counter-resource"
// keywords: "resource, counter, stateful, interface, world"
---

# Counter Resource

This example shows how to define an interface with stateful operations, which is a common pattern for managing resources in Kettu.

## The Code

```kettu
package example:resources;

/// Counter operations interface
interface counter-ops {
    /// Create a new counter with initial value
    create: func(initial: s32) -> s32 {
        initial
    }

    /// Get current value of a counter
    get: func(counter-id: s32) -> s32 {
        counter-id
    }

    /// Increment a counter by 1
    increment: func(counter-id: s32) {
        let _ = counter-id + 1;
    }

    /// Add a value to a counter
    add: func(counter-id: s32, n: s32) {
        let _ = counter-id + n;
    }

    /// Reset counter to zero
    reset: func(counter-id: s32) {
        let _ = counter-id * 0;
    }
}

world resources-world {
    export counter-ops;
}
```

## Understanding Resources

In this simplified example, the `counter-id` acts as a handle to the counter's state. In a more advanced Kettu application, these identifiers would point to memory managed by the host environment or a specialized state management component.

- **State Management**: Functions like `increment` and `add` simulate state transitions.
- **Identifiers**: The `s32` values represent handles to external state.
- **Exporting Interfaces**: By exporting the `counter-ops` interface, we enable clients to interact with this stateful service.
