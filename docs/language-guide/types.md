# Data Types

## Primitive Types

| Type | Description |
|------|-------------|
| `bool` | Boolean (true/false) |
| `s8`, `s16`, `s32`, `s64` | Signed integers |
| `u8`, `u16`, `u32`, `u64` | Unsigned integers |
| `f32`, `f64` | Floating point |
| `char` | Unicode character |
| `string` | UTF-8 string |

## Records

Named product types with fields:

```kettu
record point {
    x: s32,
    y: s32,
}

record person {
    name: string,
    age: u8,
    active: bool,
}
```

## Variants

Tagged unions (sum types):

```kettu
variant result {
    ok(s32),
    error(string),
}

variant option {
    some(string),
    none,
}
```

### Variant Literals

Construct variant values using the `#` prefix:

```kettu
// Unqualified (inferred type)
let n = #none;
let s = #some(42);

// Qualified (explicit type)
let r = result#ok(10);
let e = option#none;
```

## Enums

Simple enumerations without payloads:

```kettu
enum color {
    red,
    green,
    blue,
}

enum status {
    pending,
    active,
    complete,
}
```

## Flags

Bit flags that can be combined:

```kettu
flags permissions {
    read,
    write,
    execute,
}
```

## Type Aliases

Create aliases for existing types:

```kettu
type user-id = u64;
type name = string;
```

## Generic Types

Built-in generic types:

```kettu
// Optional value
option<string>

// Result with error
result<s32, string>
result<_, string>  // No success value
result<s32>        // No error value

// Collections
list<u8>
tuple<s32, string, bool>
```
