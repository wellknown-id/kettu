---
// docs-meta: controls how this page appears in `kettu docs`
// section: "Language Topics"
// order: 1
// title: "Packages & Interfaces"
// file: "packages"
---
# Packages & Interfaces

## Package Declaration

Every Kettu file starts with a package declaration:

```kettu nocheck
package namespace:name;
package namespace:name@1.0.0;  // With version
```

## Interfaces

Interfaces define a collection of types and functions:

```kettu
interface math {
    add: func(a: s32, b: s32) -> s32 {
        return a + b;
    }
    
    multiply: func(a: s32, b: s32) -> s32 {
        return a * b;
    }
}
```

### Use Statements

Import types from other interfaces:

```kettu nocheck
interface consumer {
    use types.{my-type, other as alias};
    
    process: func(t: my-type);
}
```

## Worlds

Worlds define component boundaries with imports and exports:

```kettu nocheck
world my-component {
    import console;
    export math;
}
```

### World Includes

Compose worlds by including others:

```kettu nocheck
world base {
    import logging;
}

world extended {
    include base;
    export my-interface;
}
```

### Inline Imports/Exports

```kettu nocheck
world inline-example {
    import run: func();
    export get-value: func() -> s32;
}
```
