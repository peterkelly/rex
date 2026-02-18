'use strict';

const assert = require('assert');
const {
  functionArityFromType,
  splitTopLevelArrows,
  stripTopLevelWhereClause,
  stripOuterParens
} = require('../src/arity');

assert.deepStrictEqual(splitTopLevelArrows('i32 -> i32'), ['i32', 'i32']);
assert.deepStrictEqual(splitTopLevelArrows('(a -> b) -> List a -> List b'), ['(a -> b)', 'List a', 'List b']);
assert.deepStrictEqual(splitTopLevelArrows('{ f: (a -> b) } -> i32'), ['{ f: (a -> b) }', 'i32']);

assert.strictEqual(stripTopLevelWhereClause('a -> b where Eq a'), 'a -> b');
assert.strictEqual(stripTopLevelWhereClause('(a where b) -> c'), '(a where b) -> c');

assert.strictEqual(stripOuterParens('(i32 -> i32)'), 'i32 -> i32');
assert.strictEqual(stripOuterParens('(i32 -> i32) -> i32'), '(i32 -> i32) -> i32');

assert.strictEqual(functionArityFromType('i32'), 0);
assert.strictEqual(functionArityFromType('i32 -> i32'), 1);
assert.strictEqual(functionArityFromType('(a -> b) -> List a -> List b'), 2);
assert.strictEqual(functionArityFromType('(a -> b) -> List a -> List b where Functor List'), 2);

console.log('arity tests passed');
