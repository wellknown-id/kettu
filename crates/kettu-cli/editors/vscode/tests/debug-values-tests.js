const assert = require('assert');
const { collectVisibleLocals } = require('../debug-values');

const source = `package local:test;
interface tests {
    @test
    calc: func() -> bool {
        let a = 10;
        let b = 20;
        let sum = a + b;
        let ok = sum == 30;
        return ok;
    }
}`;

const localsAtSum = collectVisibleLocals(source, 4, 7);
assert.deepStrictEqual(
    localsAtSum,
    { a: 10, b: 20, sum: 30 },
    'should infer locals through arithmetic assignment'
);

const localsAtBool = collectVisibleLocals(source, 4, 8);
assert.strictEqual(localsAtBool.ok, true, 'should infer boolean comparisons');

const withUpdate = `
interface x {
    @test
    t: func() -> bool {
        let n = 1;
        n = n + 4;
        return n == 5;
    }
}
`;

const localsAfterAssign = collectVisibleLocals(withUpdate, 4, 6);
assert.strictEqual(localsAfterAssign.n, 5, 'should track assignment updates');

console.log('Debug values tests passed');
