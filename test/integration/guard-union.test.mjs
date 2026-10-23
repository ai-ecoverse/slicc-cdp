import assert from 'node:assert/strict';
import { readdir } from 'node:fs/promises';
import test from 'node:test';
import {
  canonicalSkip,
  mandatory,
  partitions,
  published,
  rowKey,
  rowsFor,
  verdictProblems,
} from './guard-set.mjs';

const ids = Object.keys(partitions);

test('the guard partitions split the published targets exactly once, each in its own test file', async () => {
  const assigned = Object.values(partitions).flat();
  const targets = [...new Set(published.vectors.flatMap((vector) => vector.targets))];
  assert.equal(new Set(assigned).size, assigned.length, 'a target is in two partitions');
  assert.deepEqual([...assigned].sort(), [...targets].sort(), 'partitions cover every target');
  const universe = rowsFor(targets).map(rowKey).sort();
  const union = ids.flatMap((id) => rowsFor(partitions[id]).map(rowKey)).sort();
  assert.equal(new Set(universe).size, universe.length, 'published row keys are unique');
  assert.deepEqual(union, universe, 'partition rows add up to the whole published set');
  for (const id of ids) assert.ok(rowsFor(partitions[id]).length > 0, `partition ${id} has rows`);
  const files = (await readdir(new URL('./', import.meta.url))).filter((file) =>
    /^guard-[a-z]\.test\.mjs$/.test(file)
  );
  assert.deepEqual(
    files.sort(),
    ids.map((id) => `guard-${id}.test.mjs`)
  );
});

test('the mandatory C2.redirect-* and C4.iframe-preattach .shared rows have no declared skip', () => {
  for (const vector of published.vectors.filter((vector) => mandatory(vector.id)))
    for (const target of vector.targets)
      assert.equal(canonicalSkip(vector, target), null, `${vector.id} ${target}`);
});

test('strict verdicts take PASS or the declared skip with its reason, and nothing else', () => {
  const declared = published.vectors.find((vector) => vector.skip);
  const plain = published.vectors.find(
    (vector) =>
      vector.kind !== 'text' &&
      !vector.skip &&
      !vector.observable &&
      !mandatory(vector.id) &&
      !vector.id.startsWith('C8.protected-bfcache-restore')
  );
  const row = (vector, target, legs, reason) => ({
    id: vector.id,
    target,
    variant: 'prompt',
    control: legs,
    guarded: legs,
    live: legs,
    ...(reason === undefined ? {} : { reason }),
  });
  assert.deepEqual(verdictProblems([row(plain, plain.targets[0], 'pass')]), []);
  assert.equal(verdictProblems([row(plain, plain.targets[0], 'skip')]).length, 1, 'no reason');
  assert.equal(
    verdictProblems([row(plain, plain.targets[0], 'skip', 'not-applicable')]).length,
    1,
    'an undeclared not-applicable'
  );
  assert.equal(
    verdictProblems([row(plain, plain.targets[0], 'skip', 'not-supported:browserSession')]).length,
    1
  );
  assert.equal(verdictProblems([row(plain, plain.targets[0], 'fail')]).length, 1);
  assert.equal(
    verdictProblems([{ ...row(plain, plain.targets[0], 'pass'), live: 'skip' }]).length,
    1,
    'a partial skip'
  );
  if (declared) {
    const { target, reason } = declared.skip;
    assert.deepEqual(verdictProblems([row(declared, target, 'skip', reason)]), []);
    if (reason === 'not-applicable')
      assert.deepEqual(verdictProblems([row(declared, target, 'skip', reason)]), []);
    else
      assert.equal(
        verdictProblems([row(declared, target, 'skip', 'not-applicable')]).length,
        1,
        'the declared reason is exact'
      );
    assert.equal(
      verdictProblems([row(declared, target, 'skip', 'not-supported:browserSession')]).length,
      1
    );
    assert.equal(verdictProblems([row(declared, target, 'skip')]).length, 1, 'no reason');
    const other = declared.targets.find((name) => name !== target);
    if (other)
      assert.equal(
        verdictProblems([row(declared, other, 'skip', 'not-applicable')]).length,
        1,
        'declared for another target only'
      );
  }
  for (const id of [
    'C2.redirect-secret',
    'C4.iframe-preattach.popup.shared',
    'C8.protected-bfcache-restore.history',
  ]) {
    const vector = { id, targets: ['ordinary'] };
    assert.equal(
      verdictProblems([{ ...row(vector, 'ordinary', 'skip', 'not-applicable') }]).length,
      1,
      `${id} is never skipped`
    );
  }
  const everything = published.vectors.flatMap((vector) =>
    vector.targets.map((target) => row(vector, target, 'skip'))
  );
  assert.equal(verdictProblems(everything).length, everything.length, 'an all-skip run fails');
});
