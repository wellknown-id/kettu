---
// docs-meta: controls how this page appears in `kettu docs`
// section: "Language Topics"
// order: 4
// title: "Expressions"
// file: "expressions"
---
# Expressions

## Literals

```kettu nocheck
42           // Integer
true false   // Boolean
"hello"      // String

// Record literal
{ x: 10, y: 20 }

// Variant literals (# prefix)
#none           // Without payload
#some(42)       // With payload
```

## Arithmetic Operators

```kettu nocheck
a + b   // Addition
a - b   // Subtraction
a * b   // Multiplication
a / b   // Division
```

## Comparison Operators

```kettu nocheck
a == b  // Equal
a != b  // Not equal
a < b   // Less than
a <= b  // Less or equal
a > b   // Greater than
a >= b  // Greater or equal
```

## Logical Operators

Short-circuit evaluation:

```kettu nocheck
a && b  // And (skips b if a is false)
a || b  // Or (skips b if a is true)
```

## If / Else

Conditional branching:

```kettu nocheck
if x > 0 {
    return x;
} else {
    return 0;
};
```

Nested conditionals:

```kettu nocheck
if x > 20 {
    return 3;
} else {
    if x > 10 {
        return 2;
    } else {
        return 1;
    };
};
```

## Function Calls

```kettu nocheck
add(1, 2)
greet("world")
get_value()
```

## Assert Expressions

Assert panics if the condition is false:

```kettu nocheck
assert 2 + 2 == 4;
assert x > 0;
```

Useful in tests:

```kettu nocheck
@test
test-math: func() -> bool {
    assert 10 / 2 == 5;
    assert 3 * 4 == 12;
    return true;
}
```

## Operator Precedence

From highest to lowest:

1. Function calls, field access
2. `*`, `/`
3. `+`, `-`
4. `<`, `<=`, `>`, `>=`
5. `==`, `!=`
6. `&&`
7. `||`

## Negation

```kettu nocheck
!true       // false
!(x > 5)    // negate a comparison
```

## Assignment

Reassign existing variables:

```kettu
let x = 10;
x = 20;      // reassignment
```

### Compound Assignment

```kettu nocheck
x += 5;   // x = x + 5
x -= 2;   // x = x - 2
```

## Field Access

Access record fields with `.`:

```kettu nocheck
let point = { x: 10, y: 20 };
point.x    // 10
point.y    // 20

// Inline
{ x: 10, y: 20 }.x  // 10
```

## Optional Chaining

Safely access fields on optional values with `?.`:

```kettu nocheck
let maybe = some({ x: 10 });
maybe?.x   // some(10) if maybe is some, none if none
```

## Try Operator

Unwrap `some`/`ok` or propagate `none`/`err`:

```kettu nocheck
let val = maybe?;  // unwraps or early-returns none
```

## Record Literals

Construct records inline:

```kettu nocheck
let point = { x: 10, y: 20 };
let r = { a: 1, b: 2, c: 3 };
```

## List Literals

See [Lists](./lists.md):

```kettu
let arr = [1, 2, 3];
arr[0]     // 1
arr[1..3]  // [2, 3]
```

## Variant Literals

See [Data Types](./types.md#variant-literals):

```kettu nocheck
let x = #some(42);
let y = #none;
let r = result#ok(10);
```

