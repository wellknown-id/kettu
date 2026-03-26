# Closures & Higher-Order Functions

## Lambdas

Anonymous functions with `|params| body` syntax:

```kettu
let double = |x| x * 2;
double(21)  // 42

let add = |a, b| a + b;
add(10, 20) // 30
```

## Closures (Captured Variables)

Lambdas capture variables from the enclosing scope:

```kettu
let x = 10;
let add-x = |n| n + x;
add-x(5)  // 15

let a = 10;
let b = 20;
let sum-with-ab = |n| n + a + b;
sum-with-ab(5)  // 35
```

## Higher-Order Functions

### `map(list, fn)` — Transform

Apply a function to every element, returning a new list:

```kettu
let arr = [1, 2, 3];
let doubled = map(arr, |x| x * 2);
// doubled == [2, 4, 6]
```

### `filter(list, fn)` — Select

Keep elements where the predicate returns true:

```kettu
let arr = [1, 10, 2, 20, 3, 30];
let big = filter(arr, |x| x > 5);
// big == [10, 20, 30]
```

### `reduce(list, init, fn)` — Fold

Accumulate a single value from a list:

```kettu
let arr = [1, 2, 3, 4, 5];
let sum = reduce(arr, 0, |acc, x| acc + x);
// sum == 15

let arr = [2, 3, 4];
let product = reduce(arr, 1, |acc, x| acc * x);
// product == 24
```

## First-Class Functions

Lambdas can be stored and passed like any value:

```kettu
let triple = |x| x * 3;
let tripled = map([1, 2, 3], triple);
// tripled == [3, 6, 9]

let pred = |x| x > 5;
let big = filter([2, 4, 6, 8, 10], pred);
// big == [6, 8, 10]
```

## Trailing Closure Syntax

When the last argument to a function is a lambda, you can place it **after** the parentheses:

```kettu
// Standard syntax
let doubled = map(arr, |x| x * 2);

// Trailing closure (equivalent)
let doubled = map(arr) |x| x * 2;
```

Works with all HOFs:

```kettu
let bigs = filter(arr) |x| x > 3;
let sum = reduce(arr, 0) |acc, x| acc + x;
```

Even functions with a single lambda argument can omit parentheses:

```kettu
let apply = |f| f(10);
let result = apply |x| x * 2;
// result == 20
```
