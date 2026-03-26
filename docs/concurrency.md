# Concurrency in Kettu

Kettu provides first-class concurrency support via the WASM Threads proposal. All concurrency primitives compile to standard WebAssembly atomics and shared memory — no runtime or GC required.

> **Prerequisite**: Compile with `kettu build --threads` to enable shared memory and thread-spawn support.

## Spawning Threads

Use `spawn { ... }` to run code on a new thread. It returns an opaque `thread-id`:

```kettu
let tid = spawn {
    // runs on a new wasi thread
    expensive_computation();
};
```

The `thread-id` type is opaque — you can't accidentally do arithmetic on it.

## Joining Threads

Two equivalent ways to wait for a thread to complete:

```kettu
// Method syntax
thread.join(tid);

// Await sugar (preferred)
await tid;
```

Both block the current thread until the spawned thread finishes, using `memory.atomic.wait32` under the hood.

## Shared Memory

### Declaring shared variables

Use `shared let` to allocate a variable in shared memory:

```kettu
shared let counter = 0;
```

This auto-allocates 4-byte aligned shared memory and initializes it with an atomic store. The variable holds an opaque handle to the memory location — you can't use it in normal arithmetic (the type checker enforces this).

### Atomic operations (explicit)

For fine-grained control, use the `atomic.*` built-ins directly:

```kettu
shared let x = 0;

atomic.store(x, 42);
let v = atomic.load(x);
atomic.add(x, 1);
atomic.sub(x, 1);
atomic.cmpxchg(x, expected, new_val);

// Low-level synchronization
atomic.wait(addr, expected, timeout);
atomic.notify(addr, count);
```

### Atomic blocks (sugar)

Wrap shared-variable operations in `atomic { ... }` for automatic desugaring:

```kettu
shared let counter = 0;

// Increment — compiles to i32.atomic.rmw.add
atomic { counter += 1; };

// Decrement — compiles to i32.atomic.rmw.sub
atomic { counter -= 5; };

// Store — compiles to i32.atomic.store
atomic { counter = 42; };

// Load — compiles to i32.atomic.load
let value = atomic { counter };
```

Non-shared variables inside `atomic { }` compile normally — only shared variables get the atomic treatment.

## Compound Assignments

`+=` and `-=` work everywhere, not just inside atomic blocks:

```kettu
let x = 10;
x += 5;   // x is now 15
x -= 3;   // x is now 12
```

Inside `atomic { }` blocks on shared variables, these are promoted to atomic read-modify-write instructions.

## Complete Example

```kettu
package local:demo;

interface counter {
    run: func() -> s32 {
        shared let total = 0;

        let t1 = spawn {
            atomic { total += 100; };
        };
        let t2 = spawn {
            atomic { total += 200; };
        };

        await t1;
        await t2;

        // total is now 300
        atomic { total };
    }
}
```

## Desugaring Reference

| Source (sugar) | Compiled WASM instruction |
|---|---|
| `shared let x = 0;` | `i32.atomic.store` + local allocation |
| `atomic { x += val; }` | `i32.atomic.rmw.add` |
| `atomic { x -= val; }` | `i32.atomic.rmw.sub` |
| `atomic { x = val; }` | `i32.atomic.store` |
| `atomic { x }` | `i32.atomic.load` |
| `await tid` | `memory.atomic.wait32` |
| `thread.join(tid)` | `memory.atomic.wait32` |
| `spawn { body }` | `thread-spawn` import + done-flag allocation |
