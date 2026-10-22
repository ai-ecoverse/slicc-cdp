import assert from 'node:assert/strict';
import test from 'node:test';
import { guardConcurrency } from './guard-fixture.mjs';

test('the full guard set defaults to a bounded browser budget', () => {
  const { parallel, targetParallel } = guardConcurrency(14, {});
  assert.equal(parallel, 4);
  assert.equal(targetParallel, 4);
  assert.equal(parallel * targetParallel, 16);
});

test('a single selected target does not expand the target budget', () => {
  assert.deepEqual(guardConcurrency(1, {}), { parallel: 4, targetParallel: 1 });
});

test('explicit certification concurrency is preserved', () => {
  assert.deepEqual(
    guardConcurrency(4, {
      SLICC_GUARD_PARALLEL: '4',
      SLICC_GUARD_TARGET_PARALLEL: '2',
    }),
    { parallel: 4, targetParallel: 2 }
  );
});

test('invalid guard concurrency cannot hang a target batch', () => {
  for (const env of [
    { SLICC_GUARD_PARALLEL: '0' },
    { SLICC_GUARD_TARGET_PARALLEL: '0' },
    { SLICC_GUARD_PARALLEL: 'not-a-number' },
    { SLICC_GUARD_TARGET_PARALLEL: '1.5' },
  ]) {
    assert.throws(() => guardConcurrency(14, env), RangeError);
  }
});
