---
// docs-meta: controls how this page appears in `kettu docs`
// section: "Advanced Topics"
// order: 4
// title: "Language Server"
// file: "lsp"
---
# Kettu LSP Hover Capabilities

This document describes what hover currently supports in `kettu-lsp`.

## Scope

Hover is provided for:

- Local symbols in the current document
- Imported symbols (cross-file), including world imports and top-level `use`
- Expression identifiers with lightweight local type inference

## What Hover Shows

### 1) Declared symbols

For declarations and references that resolve to known symbols:

- `interface`: `**interface** name`
- `world`: `**world** name`
- `func`: `**func** name(param: ty, ...) -> result_ty`
- `type` aliases/typedefs: `**type** name: ...`
- `record` / `enum` / `variant` / `resource`

### 2) Parameters in function bodies

Inside expression bodies, identifier hovers prefer parameter-level information:

- `**param** b: s32`

This avoids misleading container-level hover when cursor is on a parameter use (for example, `a * b`).

### 3) Imported symbols (cross-file)

Hover resolves imported symbols via import resolution logic used by go-to-definition:

- World imports (for example, `import helper:lib/math;`)
- Top-level `use` imports (including aliases)
- Qualified member calls (for example, `math.add(...)`, `hmath.add(...)`)

If the imported file is not open, hover can load and parse it from disk.

### 4) Local `let` inferred types

For local `let` identifiers in expression contexts, hover can show inferred types:

- `**let** x: s32`

Current inference includes:

- Literals: integer (`s32`), `bool`, `string`
- Identifier propagation from known params/locals
- Binary ops:
  - comparisons/logical => `bool`
  - arithmetic => propagated type when derivable
- Calls:
  - local function calls with known return types
  - imported qualified calls with known return types
- Field access:
  - record field types from known record schemas
- `try` / `await` / optional-chain expressions:
  - `v?` infers from `option<T>` / `result<T,E>` payloads
  - `await f` infers from `future<T>`
  - `p?.field` infers to `option<field_ty>` when `p: option<RecordType>`
- Conditional expressions:
  - `if` branch unification when both branches agree
- Match expressions:
  - arm result unification when all arms agree
  - pattern-bound payload typing for `option<T>`, `result<T,E>`, and known variant typedef payloads
  - qualified variant constructors/patterns only contribute inference when payload arity is valid (`type#case` vs `type#case(value)` / `type#case(binding)`)

## Resolution Precedence (local document)

When hovering an identifier token:

1. Parameter hover (if inside containing function and name matches parameter)
2. Named symbol hover (if identifier matches a known symbol)
3. Local `let` inferred type hover (if inferable)
4. Otherwise no hover for unresolved identifiers

When hovering non-identifier positions:

- Falls back to smallest enclosing symbol span.

## Known Limits

- Inference is intentionally lightweight (not full type-checking).
- No deep control-flow-sensitive data-flow analysis.
- Limited parsing for generic textual forms (for example, complex nested type strings may not infer in all cases).
- Unresolved identifiers intentionally produce no hover to avoid misleading output.

## Related Behavior

- Go-to-definition and imported hover share the same import-aware symbol resolution model.
- Both positive and negative behavior are covered by `kettu-lsp` unit tests.

## Quick Fixes (Code Actions)

`kettu-lsp` now exposes quick-fix code actions for checker-reported qualified variant arity diagnostics.

Current quick fixes:

- Constructor requires payload (`Case 'type#case' requires a payload`):
  - Suggests adding payload argument: `type#case` → `type#case(/* payload */)`
- Constructor forbids payload (`Case 'type#case' does not accept a payload`):
  - Suggests removing payload argument: `type#case(value)` → `type#case`
- Pattern requires payload binding (`Case 'type#case' pattern requires a binding for payload`):
  - Suggests adding binding: `type#case` → `type#case(value)`
- Pattern forbids payload binding (`Case 'type#case' pattern must not bind a payload`):
  - Suggests removing binding: `type#case(binding)` → `type#case`

These actions are produced from `kettu-checker` diagnostics and apply minimal, range-local edits.
