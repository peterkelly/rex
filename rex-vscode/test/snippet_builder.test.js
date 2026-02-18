'use strict';

const assert = require('assert');
const { buildFunctionCallTemplate } = require('../src/snippet_builder');

assert.strictEqual(buildFunctionCallTemplate('id', 'i32'), 'id');
assert.strictEqual(
  buildFunctionCallTemplate('map', '(a -> b) -> List a -> List b'),
  'map ${1:arg1} ${2:arg2}'
);
assert.strictEqual(
  buildFunctionCallTemplate('foldl', '(b -> a -> b) -> b -> List a -> b'),
  'foldl ${1:arg1} ${2:arg2} ${3:arg3}'
);
assert.strictEqual(
  buildFunctionCallTemplate('pure', 'a -> f a where Applicative f'),
  'pure ${1:arg1}'
);
assert.strictEqual(
  buildFunctionCallTemplate('map', '(a -> b) -> List a -> List b', { includeTypeComment: true }),
  'map ${1:arg1} ${2:arg2} {- map : (a -> b) -> List a -> List b -}'
);
assert.strictEqual(
  buildFunctionCallTemplate('id', 'i32', { includeTypeComment: true }),
  'id {- id : i32 -}'
);

console.log('snippet builder tests passed');
