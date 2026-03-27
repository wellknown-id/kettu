---
// docs-meta: controls how this page appears in `kettu docs`
// section: "Advanced Topics"
// order: 1
// title: "SIMD"
// file: "simd"
---
# SIMD Support

Kettu provides full WebAssembly SIMD support through an ergonomic `interpretation.op(args)` syntax pattern. SIMD operations work on 128-bit vectors (v128) with 6 typed interpretations.

## Type Interpretations

| Interpretation | Element Type   | Lanes |
| -------------- | -------------- | ----- |
| `i8x16`        | 8-bit integer  | 16    |
| `i16x8`        | 16-bit integer | 8     |
| `i32x4`        | 32-bit integer | 4     |
| `i64x2`        | 64-bit integer | 2     |
| `f32x4`        | 32-bit float   | 4     |
| `f64x2`        | 64-bit float   | 2     |

Additionally, `v128` is available for type-agnostic bitwise operations.

## Syntax

All SIMD operations follow the pattern:

```
interpretation.operation(arg1, arg2, ...)
```

### Arithmetic

```
let result = i32x4.add(a, b);    // element-wise add
let result = f64x2.mul(a, b);    // element-wise multiply
let neg = i32x4.neg(v);          // negate all lanes
let a = f32x4.abs(v);            // absolute value
```

### Splat (Broadcast)

```
let v = i32x4.splat(42);         // [42, 42, 42, 42]
let v = f64x2.splat(3.14);       // [3.14, 3.14]
```

### Lane Access

```
let x = i32x4.extract_lane(v, 2);        // get lane 2
let v2 = i32x4.replace_lane(v, 1, 99);   // set lane 1 to 99
```

### Comparisons

```
let mask = i32x4.eq(a, b);       // element-wise equality
let mask = f64x2.lt(a, b);       // element-wise less-than
```

### Bitwise (v128)

```
let r = v128.and(a, b);
let r = v128.or(a, b);
let r = v128.xor(a, b);
let r = v128.not(v);
```

### Float-Only Operations

```
let r = f32x4.div(a, b);
let r = f64x2.sqrt(v);
let r = f32x4.ceil(v);
let r = f64x2.floor(v);
```

### Tests

```
let any = v128.any_true(v);      // 1 if any bit set
let all = i32x4.all_true(v);     // 1 if all lanes non-zero
let bits = i32x4.bitmask(v);     // high bits as i32
```

## SIMD Loops

Use `simd for` to process list elements 4-at-a-time with vectorized operations:

```kettu nocheck
// Double every element (processes 4 per iteration)
simd for v in numbers {
    i32x4.mul(v, i32x4.splat(2))
}
```

Inside the loop, `v` is a `v128` containing 4 consecutive `i32` elements loaded via `v128.load`. The body expression must return a `v128`, which is stored back to the list. Elements that don't fill a complete group of 4 (the remainder) are left untouched.

## Complete Operation List

Approximately 45 operations are supported across all interpretations:


- **Arithmetic**: `add`, `sub`, `mul`, `neg`, `abs`
- **Float**: `div`, `sqrt`, `ceil`, `floor`, `trunc`, `nearest`
- **Shifts**: `shl`, `shr_s`, `shr_u`
- **Comparisons**: `eq`, `ne`, `lt_s`, `gt_s`, `le_s`, `ge_s`, `lt_u`, `gt_u`, `le_u`, `ge_u`
- **Float comparisons**: `lt`, `gt`, `le`, `ge`
- **Lane access**: `extract_lane`, `replace_lane`, `splat`, `swizzle`
- **Bitwise**: `and`, `or`, `xor`, `not`, `andnot`, `bitselect`
- **Tests**: `any_true`, `all_true`, `bitmask`, `popcnt`
- **Min/Max**: `min`, `max`
- **Average**: `avgr_u`
- **Dot product**: `dot`
- **Memory**: `load`, `store`
- **Widening**: `ext_mul_low_s`, `ext_mul_high_s`, `ext_mul_low_u`, `ext_mul_high_u`
- **Pairwise**: `ext_add_pairwise_s`, `ext_add_pairwise_u`
- **Narrowing**: `narrow_s`, `narrow_u`
- **Extending**: `extend_low_s`, `extend_high_s`, `extend_low_u`, `extend_high_u`
