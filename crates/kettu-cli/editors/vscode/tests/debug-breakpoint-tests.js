const assert = require('assert');
const { normalizePath, hasBreakpointInRange } = require('../debug-breakpoints');

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

console.log('Debug breakpoint tests passed');
