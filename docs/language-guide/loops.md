# Loops & Iteration

## While Loop

Repeats while a condition is true:

```kettu
let i = 0;
while i < 10 {
    i += 1;
};
```

## For Range Loop

Iterate over a numeric range with `to` (ascending) or `downto` (descending):

```kettu
// 0, 1, 2, 3, 4 (inclusive end)
let sum = 0;
for i in 0 to 4 {
    sum = sum + i;
};
// sum == 10
```

### Step

Control the increment with `step`:

```kettu
// 0, 2, 4, 6, 8, 10
for i in 0 to 10 step 2 {
    sum = sum + i;
};

// 0, 3, 6, 9
for i in 0 to 9 step 3 {
    process(i);
};
```

### Descending

Count down with `downto`:

```kettu
// 5, 4, 3, 2, 1
for i in 5 downto 1 {
    sum = sum + i;
};

// 10, 8, 6, 4, 2, 0
for i in 10 downto 0 step 2 {
    process(i);
};
```

## For-Each Loop

Iterate over list elements:

```kettu
let arr = [10, 20, 30];
let sum = 0;
for item in arr {
    sum = sum + item;
};
// sum == 60
```

## Break & Continue

Exit or skip iterations:

```kettu
// Break exits the loop
while true {
    break;
};

// Conditional break
for i in 0 to 100 {
    break if i > 10;
};

// Continue skips to next iteration
for i in 0 to 10 {
    continue if i == 5;
    process(i);
};
```

## SIMD Loops

For vectorized processing, see [SIMD](../simd.md):

```kettu
simd for v in numbers {
    i32x4.mul(v, i32x4.splat(2))
};
```
