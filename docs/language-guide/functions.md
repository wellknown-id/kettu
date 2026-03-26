# Functions

## Function Signatures

Functions are declared with parameters and optional return type:

```kettu
// No parameters, no return
do-something: func();

// Parameters
greet: func(name: string);

// Return type
get-value: func() -> s32;

// Parameters and return
add: func(a: s32, b: s32) -> s32;
```

## Function Bodies (Kettu Extension)

Kettu extends WIT by allowing function implementations:

```kettu
interface math {
    add: func(a: s32, b: s32) -> s32 {
        return a + b;
    }
    
    max: func(a: s32, b: s32) -> s32 {
        if a > b { a } else { b }
    }
}
```

## Statements

### Let Bindings

```kettu
let x = 10;
let result = a + b;
let flag = x > 0;
```

### Return

```kettu
return 42;
return x + y;
return;  // For functions with no return type
```

### Expression Statements

The last expression in a function body is implicitly returned:

```kettu
max: func(a: s32, b: s32) -> s32 {
    if a > b { a } else { b }  // Implicit return
}
```

## Async Functions

Mark functions as async:

```kettu
fetch: async func(url: string) -> string;
```
