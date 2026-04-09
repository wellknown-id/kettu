# Contracts

Kettu supports *code-by-contract* features that allow developers to attach
runtime and compile-time constraints to function parameters and type aliases.

## Overview

Constraints are expressed using `where` clauses.  The compiler uses them in
three ways:

1. **Runtime assertions.**  For every constrained parameter, an assertion is
   injected at the head of the function body.  If the assertion fails at
   runtime the module traps via the `kettu:contract/fail` import.

2. **Compile-time analysis.**  When a constrained function is called with
   constant arguments the checker evaluates the constraint at compile time and
   emits an error (or warning) if the constraint is violated (or cannot be
   proven satisfied).

3. **Metadata emission.**  Every constrained function is recorded in a
   `kettu-contracts` custom section of the produced WASM module as a JSON blob
   so that Kettu-aware tooling can inspect contracts without source access.

Constraint evaluation is *transitive*: if function *A* calls *B* which calls
constrained *C*, the checker propagates *C*'s constraints through *B* and
surfaces violations at *A*'s call site.

---

## Function parameter constraints

A `where` expression may follow the type in any function parameter:

```kettu
test-bounds: func(small: s32, big: s32 where big > small) -> result<bool, string> {
    result#ok(true)
}
```

Multiple parameters can be constrained independently:

```kettu
test-ten-items-or-less: func(
    count: s32 where count < 10,
    items: list<s32>,
) -> result<bool, string> {
    result#ok(true)
}
```

### Compile-time call-site checking

When a constrained function is called with constants the checker evaluates the
constraint immediately:

```kettu
@test
test-bounds-called: func() -> bool {
    let big = 10;
    let small = 20;
    test-bounds(small, big);
    //  ^ error: big does not satisfy the constraint "big (10) > small (20)" on test-bounds
    true
}
```

### Propagation through unconstrained parameters

When a constrained function is called with a non-constant argument the checker
issues a warning and suggests using a guard:

```kettu
@test-helper
call-test-bounds: func(somesmall: s32) -> bool {
    let big = 10;
    test-bounds(somesmall, big);
    //  ^ warning: big may not satisfy the constraint "big (10) > small (somesmall)"
    //             because somesmall is an unconstrained parameter,
    //             test-bounds must be called with a guard
    guard let mustbetrue = test-bounds(somesmall, big) else {
        return result#err("constraint failed");
    };
    mustbetrue
}
```

### Transitive propagation

Constraints flow through intermediate calls.  In the example below,
`call-test-bounds` inherits `test-bounds`' constraint, so the call from
`test-bounds-called-again` is checked against the original constraint:

```kettu
@test
test-bounds-called-again: func() -> bool {
    let small = 10;
    call-test-bounds(small);
    //  ^ error: small does not satisfy the constraint "big (10) > small (10)"
    //           on test-bounds (via call-test-bounds)
    true
}
```

---

## Type alias constraints

A `where` clause can be attached to a type alias.  The special identifier `it`
refers to the value being constrained:

```kettu
type length = s32 where it > 0;
```

### Developer convenience

When a constrained type alias is used as a function parameter the constraint is
automatically derived — the developer does not need to repeat it:

```kettu
use-length: func(l: length) -> result<bool, string> {
    // equivalent to: func(l: s32 where l > 0) -> ...
    result#ok(true)
}
```

The checker treats this exactly as if the parameter had an explicit `where`
clause, including call-site checking and transitive propagation.

### Return type constraints

When a function's *return type* is a constrained alias the codegen injects a
runtime guard that checks the return value before the function returns:

```kettu
type positive = s32 where it > 0;

make-positive: func(x: s32) -> positive {
    return x;  // runtime trap if x <= 0
}
```

---

## Hush comments (expected-error syntax)

Inside functions marked `@test` or `@test-helper` a comment beginning with
`///` followed by whitespace and a caret `^` suppresses a matching diagnostic:

```kettu
@test
example: func() -> bool {
    test-bounds(20, 10);
    ///    ^ big does not satisfy the constraint "big (10) > small (20)" on test-bounds
    true
}
```

Matching rules:

- The comment must appear on the line **after** the expression that produces
  the diagnostic.
- The `^` column must fall within the span of the expression.
- The text after `^` must match the diagnostic message exactly.
- A matching diagnostic is emitted at **Information** level instead of
  **Error**.

---

## Runtime behaviour

### Contract fail import

Every module that contains constrained functions imports
`kettu:contract/fail(ptr: i32, len: i32) -> ()`.  The injected assertion code
calls this import with a UTF-8 error message before executing `unreachable`.

### Custom section

A `kettu-contracts` custom section is emitted containing JSON metadata:

```json
{
  "version": 1,
  "functions": {
    "test-bounds": [
      { "name": "big", "type": "s32", "constraint": "big > small" }
    ]
  }
}
```

---

## Full example

```kettu
package local:contract-tests;

interface contract-tests {

    /// Example 1: A simple parameter constraint.
    @test
    test-bounds: func(small: s32, big: s32 where big > small) -> result<bool, string> {
        result#ok(true)
    }

    /// Example 2: Multiple constrained parameters.
    @test
    test-ten-items-or-less: func(count: s32 where count < 10, items: list<s32>) -> result<bool, string> {
        result#ok(true)
    }

    /// Example 3: Compile-time call-site violation.
    @test
    test-bounds-called: func() -> bool {
        let big = 10;
        let small = 20;
        test-bounds(small, big);
        ///                ^ big does not satisfy the constraint "big (10) > small (20)" on test-bounds
        true
    }

    /// Example 4: Propagation with an unconstrained parameter.
    @test-helper
    call-test-bounds: func(somesmall: s32) -> bool {
        let big = 10;
        test-bounds(somesmall, big);
        ///                    ^ big may not satisfy the constraint "big (10) > small (somesmall)" because somesmall is an unconstrained parameter, test-bounds must be called with a guard
        guard let mustbetrue = test-bounds(somesmall, big) else {
            return result#err("constraint failed");
        };
        mustbetrue
    }

    /// Example 5: Transitive constraint violation.
    @test
    test-bounds-called-again: func() -> bool {
        let small = 10;
        call-test-bounds(small);
        true
    }
}

world tests {
    export contract-tests;
}
```
