---
// docs-meta: controls how this page appears in `kettu docs`
// section: "Language Topics"
// order: 11
// title: "Testing"
// file: "testing"
---
# Testing

Kettu has a built-in test framework.

## Test Functions

Mark functions with `@test`:

```kettu
interface math-tests {
    @test
    test-addition: func() -> bool {
        return 2 + 3 == 5;
    }
    
    @test
    test-comparison: func() -> bool {
        if 10 > 5 { true } else { false }
    }
}
```

**Requirements:**
- Must take no parameters
- Must return `bool`
- Return `true` to pass, `false` to fail

## Assert Expression

Use `assert` for cleaner tests:

```kettu
@test
test-math-operations: func() -> bool {
    assert 2 + 2 == 4;
    assert 10 - 3 == 7;
    assert 6 * 7 == 42;
    assert 15 / 3 == 5;
    return true;
}
```

If an assert fails, the WASM module traps (crashes).

## Running Tests

```bash
# Run all tests in a file
kettu test math_test.kettu

# Filter by test name
kettu test math_test.kettu --filter addition

# Run tests in a directory (recursive)
kettu test tests/
```

## Output

```
Running 4 test(s) in math_test.kettu...

  ✓ test-addition (0.1ms)
  ✓ test-subtraction (0.1ms)
  ✓ test-multiplication (0.1ms)
  ✗ test-division (0.2ms)
      Test failed (returned false)

Results: 3 passed, 1 failed
```

## Example Test File

```kettu
// math_test.kettu
package example:math-tests;

interface calculator-tests {
    @test
    test-arithmetic: func() -> bool {
        let sum = 10 + 20;
        let diff = 50 - 30;
        assert sum == 30;
        assert diff == 20;
        return true;
    }
    
    @test
    test-comparison: func() -> bool {
        assert 10 > 5;
        assert 3 <= 3;
        assert 7 != 8;
        return true;
    }
    
    @test
    test-logic: func() -> bool {
        assert true && true;
        assert false || true;
        assert !(false && true) == true;
        return true;
    }
}
```
