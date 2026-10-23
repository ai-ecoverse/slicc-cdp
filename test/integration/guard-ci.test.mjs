import assert from 'node:assert/strict';
import test from 'node:test';
import { assertGuardReport, canonicalSkip, partitions, published } from './guard-set.mjs';
import { syntheticReport } from './guard-test-evidence.mjs';

function document(id = 'a') {
  const targets = partitions[id];
  const set = {
    ...published,
    vectors: published.vectors
      .map((vector) => ({
        ...vector,
        targets: vector.targets.filter((target) => targets.includes(target)),
      }))
      .filter((vector) => vector.targets.length),
  };
  return { ...syntheticReport(set, 'pure-pipeline-head'), selection: { targets } };
}

test('ordinary pipeline guard files retain strict complete-key behavioural and subject validation', () => {
  for (const id of Object.keys(partitions))
    assertGuardReport(document(id), id, 'pure-pipeline-head');
});

for (const defect of [
  'failed control',
  'failed guarded',
  'incomplete live',
  'unsupported skip',
  'eligible applicability forgery',
  'C2 skip',
  'shared iframe skip',
  'BFCache skip',
  'false producer pass',
  'wrong corpus',
  'wrong scope',
  'wrong head',
  'wrong selection',
  'missing row',
  'duplicate row',
  'extra row',
]) {
  test(`guard report rejects ${defect} rather than accepting a complete count`, () => {
    const doc = document();
    const id =
      defect === 'shared iframe skip'
        ? 'C4.iframe-preattach.blank.shared'
        : defect === 'BFCache skip'
          ? 'C8.protected-bfcache-restore'
          : defect === 'unsupported skip'
            ? 'C1.href'
            : 'C2.redirect-body';
    const row = doc.results.find((row) => row.id === id);
    if (defect === 'failed control') row.control = 'fail';
    if (defect === 'failed guarded') row.guarded = 'fail';
    if (defect === 'incomplete live') row.live = 'incomplete';
    if (defect.includes('skip') || defect === 'eligible applicability forgery') {
      row.control = row.guarded = row.live = 'skip';
      row.reason =
        defect === 'eligible applicability forgery'
          ? 'not-applicable'
          : 'not-supported:browserSession';
    }
    if (defect === 'false producer pass') doc.pass = false;
    if (defect === 'wrong corpus') doc.set = 'other corpus';
    if (defect === 'wrong scope') doc.guard = 'other guard';
    if (defect === 'wrong head') doc.sha = 'other head';
    if (defect === 'wrong selection') doc.selection.targets = [];
    if (defect === 'missing row') doc.results.pop();
    if (defect === 'duplicate row') doc.results.push(doc.results[0]);
    if (defect === 'extra row') doc.results.push({ ...doc.results[0], id: 'invented' });
    assert.throws(() => assertGuardReport(doc, 'a', 'pure-pipeline-head'));
  });
}

test('observable applicability keeps its canonical published reason without serialized rewrites', () => {
  const vector = published.vectors.find((vector) =>
    Object.values(vector.observable ?? {}).includes(false)
  );
  const target = Object.keys(vector.observable).find(
    (target) => vector.observable[target] === false
  );
  assert.equal(canonicalSkip(vector, target), 'not-applicable');
});
