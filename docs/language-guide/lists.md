---
// docs-meta: controls how this page appears in `kettu docs`
// section: "Language Topics"
// order: 7
// title: "Lists & Collections"
// file: "lists"
// preamble-start
//   let arr = [10, 20, 30];
// preamble-end
---
# Lists & Collections

## List Literals

Create lists with square brackets:

```kettu
let arr = [10, 20, 30];
let single = [42];
let nums = [1, 2, 3, 4, 5];
```

## Indexing

Access elements by index (0-based):

```kettu
let arr = [10, 20, 30];
arr[0]   // 10
arr[1]   // 20

// Computed index
let idx = 2;
arr[idx] // 30
```

## Slicing

Extract a sub-list with `[start..end]`:

```kettu
let arr = [10, 20, 30, 40, 50];
let sub = arr[1..4];
// sub == [20, 30, 40], length 3
```

## Built-in Functions

### `list-len(list)` — Length

```kettu
let arr = [1, 2, 3, 4, 5];
list-len(arr)  // 5
```

### `list-set(list, index, value)` — Mutate Element

Modifies the element at `index` in-place:

```kettu
let arr = [10, 20, 30];
list-set(arr, 1, 99);
arr[1]  // 99
```

### `list-push(list, value)` — Append

Returns a **new list** with the value appended (original unchanged):

```kettu
let arr = [10, 20];
let arr2 = list-push(arr, 30);
list-len(arr2)  // 3
arr2[2]         // 30
list-len(arr)   // 2 (original unchanged)
```

## Iteration

### For-Each

```kettu
let arr = [10, 20, 30];
let sum = 0;
for item in arr {
    sum = sum + item;
};
```

### Higher-Order Functions

See [Closures & HOFs](./closures.md) for `map`, `filter`, `reduce`.
