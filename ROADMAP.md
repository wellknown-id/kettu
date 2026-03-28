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

### Phase 9b: Guard Syntax ✓

- [x] Parse `guard <condition> else { ... };` statements
- [x] Parse `guard let name = value else { ... };` optional/result binding guards
- [x] Require boolean guard conditions
- [x] Unwrap `option<T>` / `result<T, E>` payloads into post-guard bindings
- [x] Require the `else` block to exit the current scope
- [x] Lower guard bodies for `return`, `break`, and `continue`
- [x] Add parser, checker, and codegen coverage

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
- [x] `thread.join(tid)` — blocks until spawned thread completes (atomic wait/notify protocol)

### Phase 13e: Ergonomic Atomics ✓

Foundation (C): `shared<dtype>` with method syntax. Sugar (B): `shared let` + `atomic { }` blocks.

- [x] `shared let x = 0;` → auto-allocates 4-byte aligned shared memory, emits `i32.atomic.store`
- [x] `atomic { ... }` block expression → compiles body, leaves final expr value on stack
- [x] All 7 atomic ops codegen: `AtomicLoad`, `AtomicStore`, `AtomicAdd`, `AtomicSub`, `AtomicCmpxchg`, `AtomicWait`, `AtomicNotify`
- [x] `CheckedType::Shared` — opaque type prevents accidental arithmetic on shared handles
- [x] Parser: `SharedLetStmt`, `AtomicBlockExpr` grammar rules + CST→AST conversion
- [x] 14 concurrency tests (5 parser, 6 checker, 3 codegen)[^1]

### Phase 13g: Syntactic Sugar for Thread Ops ✓

- [x] Compound assignments: `x += val;` / `x -= val;` (`Statement::CompoundAssign`)
- [x] Atomic block desugaring: `atomic { counter += 1; }` → `i32.atomic.rmw.add`
- [x] Atomic block desugaring: `atomic { counter }` → `i32.atomic.load`
- [x] Atomic block desugaring: `atomic { counter = val; }` → `i32.atomic.store`
- [x] `SharedLet` now allocates WASM local for memory offset (enables desugaring)
- [x] `await tid` → `thread.join(tid)` via `memory.atomic.wait32`
- [x] 6 new tests (3 parser, 3 codegen)

### Phase 14: SIMD ✅

- [x] `v128` type support
- [x] Vector operations (`i32x4.add`, `f32x4.mul`, etc.) — all 6 interpretations
- [x] Lane operations (`i32x4.extract_lane`, `i32x4.replace_lane`)
- [x] SIMD-friendly loops: `simd for v in list { body }` — vectorized v128 load/store
- [x] WASM SIMD proposal integration — ~200 instruction mappings

### Phase 15: Auto-Vectorization

- [ ] Loop analysis — detect induction variables, trip counts, memory access patterns
  - [x] Groundwork in codegen for literal range direction/step/trip counts
  - [x] Groundwork in codegen for contiguous `for item in list` access facts
  - [ ] Generalize analysis beyond literal ranges and simple list iteration shapes
- [ ] Dependence analysis — prove iterations are independent (no loop-carried deps)
- [ ] Cost model — decide if vectorization is profitable
- [ ] Automatic rewrite of scalar `for item in list` loops to SIMD when safe
- [ ] Scalar epilogue generation for remainders in auto-vectorized loops

### Phase 16: CLI Enhancements

- [x] `kettu docs` — embedded, navigable language guide (browse, search, doc-testing)
- [x] `kettu mcp` — Model Context Protocol server over stdio
  - [x] Expose compiler tools: check, parse, emit-wit, docs search, docs read
  - [x] JSON-RPC 2.0 with MCP initialize/tools/list/tools/call lifecycle
  - [x] VS Code extension integration: auto-register as MCP server for AI/LM chat contexts
  - [x] VS Code LM tool parity: register the existing `parse` MCP tool alongside the other chat tools

### Phase 16b: Debugger

- [x] Source mapping to wasm — emit Kettu→wasm location map (debug info) and surface it to DAP so stepping and stack lines match optimized builds
- [x] Integration tests for release debugging — debug a `--release` build and assert DAP stack and line mappings align with source
- [x] Data inspection and `evaluate` support — expose closure captures and locals in the Variables pane; add `evaluate` support for simple expressions behind a flag
- [x] Tests for captures and `evaluate` — assert captures appear in Variables and `evaluate` returns expected values
- [x] Nested closures and multi-breakpoint flows — preserve the correct top frame and line order when stepping between nested closures and multiple breakpoints
- [x] Tests for nested closure stepping — use a fixture with nested closures and back-to-back breakpoints, asserting frame names and monotonic line progression

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
