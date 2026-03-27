const assert = require('assert');
const { normalizePath, hasBreakpointInRange, getBreakpointLinesInRange } = require('../debug-breakpoints');

const breakpoints = new Map();
breakpoints.set(normalizePath('/tmp/sample.kettu'), new Set([3, 10, 21]));

assert.strictEqual(
    hasBreakpointInRange(breakpoints, '/tmp/sample.kettu', 1, 2),
    false,
    'should not match when no lines are in range'
);

assert.strictEqual(
    hasBreakpointInRange(breakpoints, '/tmp/sample.kettu', 2, 10),
    true,
    'should match when breakpoint falls inside start/end range'
);

assert.strictEqual(
    hasBreakpointInRange(breakpoints, '/tmp/sample.kettu', 22, 18),
    true,
    'should handle reversed ranges by normalizing min/max'
);

assert.strictEqual(
    hasBreakpointInRange(breakpoints, '/tmp/other.kettu', 1, 100),
    false,
    'should not match for different files'
);

assert.deepStrictEqual(
    getBreakpointLinesInRange(breakpoints, '/tmp/sample.kettu', 2, 12),
    [3, 10],
    'should return sorted breakpoint lines within the requested range'
);

assert.deepStrictEqual(
    getBreakpointLinesInRange(breakpoints, '/tmp/sample.kettu', 30, 40),
    [],
    'should return empty list when no breakpoints are in range'
);

console.log('Debug breakpoint tests passed');
