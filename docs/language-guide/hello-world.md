---
// docs-meta: controls how this page appears in `kettu docs`
// section: "Getting Started"
// order: 0
// title: "Hello World"
// file: "hello-world"
// keywords: "hello, start, getting started, introduction, tutorial, beginner, quickstart"
---
# Hello World

A quick tour of Kettu — a typed language that compiles to WebAssembly Components.

## Your First Component

Every Kettu file declares a **package** (a namespace for your code) and one or
more **interfaces** containing types and functions:

```kettu
package example:hello;

/// A simple greeter interface.
interface greeter {
    /// Return a friendly greeting.
    greet: func(name: string) -> string {
        return "Hello, " + name + "!";
    }
}

/// The world defines what this component exports.
world hello-world {
    export greeter;
}
```

Build it with:

```text
kettu build hello.kettu
```

This produces a `.wasm` WebAssembly Component you can run in any WASI-compatible
runtime.

## Data Types at a Glance

Kettu supports records, variants, enums, and flags — all mapped directly to the
WebAssembly Component Model:

```kettu
package example:types-demo;

interface demo-types {
    // A record is a named struct.
    record point {
        x: s32,
        y: s32,
    }

    // An enum is a simple set of named values.
    enum color {
        red,
        green,
        blue,
    }

    // Flags are combinable bit fields.
    flags permissions {
        read,
        write,
        execute,
    }

    // Functions can work with these types.
    origin: func() -> point {
        return point { x: 0, y: 0 };
    }
}
```

## Control Flow

Kettu has `if`/`else` expressions, `for` loops over lists, and `while` loops:

```kettu nocheck
interface examples {
    // If/else as an expression.
    abs: func(n: s32) -> s32 {
        if n < 0 {
            return 0 - n;
        } else {
            return n;
        };
        return n;
    }

    // Ternary-style inline if.
    max: func(a: s32, b: s32) -> s32 {
        return if a > b { a } else { b };
    }

    // For loop over a list.
    sum: func(values: list<s32>) -> s32 {
        let total = 0;
        for v in values {
            total = total + v;
        };
        return total;
    }
}
```

## Pattern Matching

The `match` expression destructures variants and values:

```kettu nocheck
interface matching {
    // Match on variant cases.
    describe: func(val: option<s32>) -> string {
        return match val {
            some(n) => "got: " + s32.to-string(n),
            none    => "nothing",
        };
    }

    // Match on integer values.
    day-name: func(day: u8) -> string {
        return match day {
            1 => "Monday",
            2 => "Tuesday",
            3 => "Wednesday",
            _ => "other",
        };
    }
}
```

## String Interpolation

Use `${ }` inside strings to embed expressions:

```kettu nocheck
interface strings {
    greet: func(name: string, age: u8) -> string {
        return "Hi ${name}, you are ${u8.to-string(age)} years old!";
    }
}
```

## What's Next?

Run `kettu docs` to browse the full language guide, or dive into a topic:

- [Packages & Interfaces](packages.md) — modules, worlds, imports/exports
- [Data Types](types.md) — records, variants, enums, flags, generics
- [Expressions](expressions.md) — operators, comparisons, string interpolation
- [Functions & Closures](functions.md) — higher-order functions, trailing closures
- [Loops & Iteration](loops.md) — for, while, loop control
