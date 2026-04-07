I want to add a code by contract features to kettu language. The first of these contracts I would like to explore is the function parameter constraint. The feature would allow a developer to write "where ..." expressions behind the type specifier in a function signature and kettu would use this information in two ways. At runtime it would inject assertions at the head of the function body to bail out if a constraint is not met, and secondly this feature would be embedded as metadata in built wasm modules (as well as being available in source of course) so that the kettu compiler can analyse code paths where constants would violate constraints. The comptime nature of the contraints would naturally allow static analyse to surface constraints in dependencies thereby implicitly constraining callees that pass constants through etc as seen in Example 3 and Example 4.

Additionally in these examples we are introducing a new expected-error syntax that applies in tests only. In a test marked with @test or test helper marked @test-helper a comment that starts with three slashes /// followed by any whitespace and up caret ^ should be interpreted by the compiler as a "hush" signal for the error it marks. That error should be emitted at the "Information" level if the text corresponds with the comment text, otherwise it is an error as usual. This is to allow developers to test contracts correctly prevent compilation without yielded an error. This syntax should only work inside the body of a function marked with the test or test-helper attribute @test or @test-helper.

```kettu
package local:contract-tests;

interface contract-tests {

    /// Example 1: A simple parameter constraint.
    @test
    test-bounds: func(small: s32, big: s32 where big > small)-> bool {
        true
    }

    /// Example 2: Another simple parameter constraint.
    @test
    test-ten-items-or-less: func(count: s32 where count < 10, items: list<s32>)-> bool {
        let x = 10;
        true
    }

    /// Example 3: An implicit parameter constraint.
    @test
    test-bounds-called: func() -> bool {
        let big = 10;
        let small = 20;
        test-bounds(small, big);
        ///                ^ big does not satisfy the constraint "big (10) > small (20)" on test-bounds
    }

    /// Example 4: Another implicit parameter constraint.
    @test-helper
    call-test-bounds: func(somesmall: s32) -> bool {
        let big = 10;
        test-bounds(somesmall, big);
        ///                    ^ big may not satisfy the constraint "big (10) > small (somesmall)" because somesmall is an unconstrained parameter, test-bounds must be called with a guard
        guard let mustbetrue = test-bounds(somesmall, big) else {
            return false;
        };
        mustbetrue;
    }
    @test
    test-bounds-called-again: func() -> bool {
        let small = 10;
        call-test-bounds(small);
        ///              ^ small does not satisfy the constraint "big (10) > small (10)" on test-bounds (via call-test-bounds)
    }
}

world tests {
    export contract-tests;
}
```