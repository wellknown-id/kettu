---
// docs-meta: controls how this page appears in `kettu docs`
// section: "Language Topics"
// order: 12
// title: "Feature Gates"
// file: "gates"
// keywords: "gate, annotation, since, deprecated, unstable, feature, version, stability"
---
# Feature Gates

Feature gates control API stability and deprecation.

## @since

Mark when a feature was introduced:

```kettu nocheck
interface api {
    @since(version = 1.0.0)
    stable-function: func();
    
    @since(version = 2.0.0)
    newer-function: func();
}
```

## @deprecated

Mark features as deprecated:

```kettu
interface legacy {
    @deprecated(version = 2.0.0)
    old-function: func();
    
    new-function: func();
}
```

## @unstable

Mark experimental features:

```kettu nocheck
interface experimental {
    @unstable(feature = async-io)
    async-read: func() -> string;
}

// Multiple features
@unstable(feature = preview1, feature = wasi)
```

## Combining Gates

Gates can be combined:

```kettu nocheck
@since(version = 1.5.0)
@deprecated(version = 2.0.0)
legacy-api: func();

@since(version = 2.0.0)
@unstable(feature = preview)
experimental-api: func();
```

## Applying to Types

Gates work on any item:

```kettu nocheck
@since(version = 1.0.0)
record point {
    x: s32,
    y: s32,
}

@deprecated(version = 2.0.0)
enum old-status {
    pending,
    done,
}

@unstable(feature = resources)
resource handle {
    constructor();
}
```

## @test

Special gate for test functions:

```kettu nocheck
@test
test-example: func() -> bool {
    return true;
}
```

Test functions:
- Are not exported in WIT output
- Are only run by `kettu test`
- Must return `bool`
