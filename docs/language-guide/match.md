---
// docs-meta: controls how this page appears in `kettu docs`
// section: "Language Topics"
// order: 6
// title: "Pattern Matching"
// file: "match"
---
# Pattern Matching

## Match Expression

Match a value against variant patterns:

```kettu nocheck
let result = match value {
    #ok => 42,
    #err => 0,
};
```

Match is an expression — it returns a value.

## Payload Binding

Extract data from variant payloads:

```kettu nocheck
let v = #ok(42);
let result = match v {
    #ok(x) => x,       // x binds to 42
    #err(e) => 0,
};
// result == 42
```

Use the bound variable in the arm body:

```kettu nocheck
let v = #ok(21);
match v {
    #ok(x) => x * 2,   // 42
    #err(e) => 0,
};
```

## Wildcard Pattern

The `_` pattern matches anything:

```kettu nocheck
match status {
    #ok => handle_success(),
    _ => 0,               // catch-all
};
```

## Multiple Arms

```kettu nocheck
let v = #err(99);
let code = match v {
    #ok(x) => 0,
    #err(code) => code,   // code binds to 99
};
```

## Match with Variant Literals

Works naturally with variant constructors:

```kettu nocheck
let maybe = #some(42);
match maybe {
    #some(v) => v,
    #none => 0,
};
```
