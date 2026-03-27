---
// docs-meta: controls how this page appears in `kettu docs`
// section: "Advanced Topics"
// order: 3
// title: "Async/Await"
// file: "wasip3"
---
# WASI Preview 3 Support

Kettu supports WASI Preview 3 async functions via the `--wasip3` flag. This enables stackless async/await with proper Component Model integration.

## Usage

```bash
kettu build myfile.kettu --wasip3 -o output.wasm
```

## How It Works

When `--wasip3` is enabled for async functions:

1. **Entry functions** return a packed `(status, subtask_or_result)` tuple
2. **Callback exports** are generated for resumption (e.g., `foo$callback`)
3. **task.return** is called to signal completion

## Host Requirements

### The `canon-async` Interface

WASIP3 components compiled by Kettu import a `canon-async` interface that the host runtime must provide. This interface contains the async primitive functions:

```wit
interface canon-async {
    task-return: func(val: s32);
    waitable-set-new: func() -> s32;
    waitable-set-wait: func(ws: s32, out-ptr: s32) -> s32;
    subtask-drop: func(subtask: s32);
}
```

| Function            | Purpose                                                   |
| ------------------- | --------------------------------------------------------- |
| `task-return`       | Signal that an async function has completed with a result |
| `waitable-set-new`  | Create a new waitable set for tracking async operations   |
| `waitable-set-wait` | Block until an event occurs in the waitable set           |
| `subtask-drop`      | Clean up a completed subtask                              |

### Why This Interface?

The Component Model's async ABI uses "canon built-in" functions (like `$async::task.return`) that standard tooling (`wit-component`) doesn't yet support directly. Kettu works around this by:

1. Defining a regular WIT interface with the same functions
2. Injecting it into the component's WIT
3. Using fully-qualified import names the host can resolve

### Implementing in Wasmtime

When instantiating a Kettu WASIP3 component, provide the `canon-async` interface:

```rust
// Pseudocode - actual API depends on Wasmtime version
linker.instance("example:mypackage/canon-async")?
    .func_wrap("task-return", |val: i32| {
        // Signal task completion with result
    })?
    .func_wrap("waitable-set-new", || -> i32 {
        // Create and return waitable set handle
    })?
    .func_wrap("waitable-set-wait", |ws: i32, out_ptr: i32| -> i32 {
        // Block until event, write event info to out_ptr
    })?
    .func_wrap("subtask-drop", |subtask: i32| {
        // Clean up subtask resources
    })?;
```

## Status Codes

WASIP3 async functions return status codes in the low 4 bits:

| Status  | Value | Meaning                                |
| ------- | ----- | -------------------------------------- |
| DONE    | 0     | Function completed, result available   |
| STARTED | 1     | Subtask started, running in background |
| WAIT    | 2     | Function blocked, needs to wait        |

## Future Work

When WASI 0.3 stabilizes (expected late 2025), the `canon-async` interface will be replaced by native Component Model async primitives. The current approach is an interim solution that enables development and testing of async Kettu code against prototype runtimes.
