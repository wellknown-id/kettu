---
// docs-meta: controls how this page appears in `kettu docs`
// section: "Language Topics"
// order: 9
// title: "Strings"
// file: "strings"
// preamble-start
//   let name = "kettu";
// preamble-end
// keywords: "string, text, concatenation, interpolation, length, char"
---
# Strings

## String Literals

```kettu
let greeting = "hello";
let empty = "";
```

## Concatenation

Join strings with `+`:

```kettu
let s = "hello" + " " + "world";
// s == "hello world"
```

## String Interpolation

Embed expressions inside strings with `{}`:

```kettu
let name = "kettu";
let msg = "Hello, {name}!";
// msg == "Hello, kettu!"

let x = 42;
let info = "The answer is {x}";
```

## Built-in Functions

### `str-len(s)` — Length

Returns the byte length of a string:

```kettu
str-len("hello")  // 5
str-len("")        // 0
```

### `str-eq(a, b)` — Equality

Compares two strings for equality:

```kettu
let a = "test";
str-eq(a, "test")  // true
str-eq(a, "other") // false
```
