# Kettu Roadmap

## Completed Phases

### Phase 1-5: Core Language ✓

- Expressions, control flow, loops
- Records and variants with match
- Strings and interpolation
- Arrays and list operations

### Phase 6: Higher-Order Functions ✓

- `map`, `filter`, `reduce` built-ins
- Rust-style lambdas `|x| expr`

### Phase 6b: First-Class Functions ✓

- Lambdas as values
- `call_indirect` for dynamic dispatch
- Pass functions to HOFs

---

## Future Phases

### Phase 7: Closures ✓

- [x] Capture analysis (identify free variables)
- [x] Closure cell allocation (heap-stored captures)
- [x] Pass environment with function table index
- [x] Compile-time closure tracking for call sites
- [x] **Trailing closure syntax**: `func(args) |x| expr`
- [x] **Parenthesis-free**: `func |x| expr`
- [x] Fix higher-order lambda function table indexing

### Phase 8: Async/Await ✓

- [x] Add `async`, `await` keywords to lexer
- [x] Parse `async func` declarations
- [x] Parse `await expr` expressions
- [x] Add `future<T>` and `stream<T>` type support
- [x] Codegen with WASIp3 async primitives (--wasip3 flag)
- [x] State machine coordination with callback exports

### Phase 9: Option/Result Ergonomics ✓

- [x] Add `some(x)` / `none` constructors
- [x] Add `ok(x)` / `err(e)` constructors
- [x] Optional chaining: `x?.field`
- [x] Try operator: `expr?`

### Phase 10: Modules & Imports ✓

- [x] Multi-file compilation
- [x] Import resolution (`use pkg:name/interface;`)
- [x] Qualified function calls (`interface.func()`)

### Phase 11: Advanced Types ✓

- [x] Generic types: `record pair<T> { a: T, b: T }`
- [x] Generic functions: `swap<T>: func(a: T, b: T) -> tuple<T, T>`
- [x] Monomorphization for WIT emission
- [ ] Trait-like interfaces (future)
- [ ] Associated types (future)

### Phase 12: Resources ✓

- [x] Resource type codegen
- [x] Constructor method ([constructor]resource-name)
- [x] Instance methods with implicit self param
- [x] Static methods

### Phase 13: Threads ✓

- [x] Atomic operations: `atomic.load`, `atomic.store`, `atomic.add`, `atomic.sub`, `atomic.cmpxchg`
- [x] Synchronization: `atomic.wait`, `atomic.notify`
- [x] `spawn { ... }` expression → extracts body, emits `wasi_thread_start` dispatcher
- [x] Opaque `thread-id` type (no accidental arithmetic)
- [x] Shared memory codegen (`--threads` flag)
- [x] WASM threads proposal integration (shared memory, atomics, `thread-spawn` import)
- [x] Ergonomic atomic syntax (Phase 13e — see below)
- [ ] `thread.join` / `thread.await`

### Phase 13e: Ergonomic Atomics ✓

Foundation (C): `shared<dtype>` with method syntax. Sugar (B): `shared let` + `atomic { }` blocks.

- [x] `shared let x = 0;` → auto-allocates 4-byte aligned shared memory, emits `i32.atomic.store`
- [x] `atomic { ... }` block expression → compiles body, leaves final expr value on stack
- [x] All 7 atomic ops codegen: `AtomicLoad`, `AtomicStore`, `AtomicAdd`, `AtomicSub`, `AtomicCmpxchg`, `AtomicWait`, `AtomicNotify`
- [x] `CheckedType::Shared` — opaque type prevents accidental arithmetic on shared handles
- [x] Parser: `SharedLetStmt`, `AtomicBlockExpr` grammar rules + CST→AST conversion
- [x] 14 concurrency tests (5 parser, 6 checker, 3 codegen)[^1]

### Phase 14: SIMD

- [ ] `v128` type support
- [ ] Vector operations (`v128.add`, `v128.mul`, etc.)
- [ ] Lane operations (`i32x4.extract_lane`)
- [ ] SIMD-friendly loops / auto-vectorization hints
- [ ] WASM SIMD proposal integration

[^1]: Syntactic sugar for atomic operations:

```kettu
// Foundation (explicit)
let counter: shared<s32> = shared(0);
counter.store(42);
let v = counter.load();
counter.add(1);
counter.cmpxchg(0, 1);

// Sugar (opt-in)
shared let counter = 0;
atomic { counter += 1; }
let v = atomic { counter };
```
