import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { guardShardPlan } from '../../scripts/cert/guard-shards.mjs';
import { assertPartitionUnion, planPartitions } from '../../scripts/guard-ci.mjs';
import { integrationArguments } from '../../scripts/integration-selection.mjs';
import { fixture } from './cert-fixture.mjs';

const vectors = JSON.parse(
  await readFile(
    new URL(import.meta.resolve('@ai-ecoverse/slicc-browsers/guard-vectors.json')),
    'utf8'
  )
);
const plan = planPartitions(vectors);
const head = 'pure-ci-subject';
const run = 'pure-ci-run';
const documents = () => {
  const original = fixture().guard.executions.positive.report;
  return plan.shards.map((shard) => ({
    id: shard.id,
    head,
    run,
    report: {
      ...original,
      sha: head,
      selection: { targets: shard.targets },
      results: original.results.filter((row) => shard.targets.includes(row.target)),
    },
  }));
};

test('normal CI reuses the cert partition library with an exact disjoint full row universe', async () => {
  assert.deepEqual(plan, guardShardPlan(vectors));
  assert.equal(assertPartitionUnion(vectors, documents(), head, run), plan.keys.length);
  const all = await integrationArguments('all');
  const general = await integrationArguments('integration');
  const focused = await integrationArguments('redirects');
  const guarded = await integrationArguments('guard');
  assert.deepEqual([...general, ...focused, ...guarded].sort(), all);
  assert.equal(new Set([...general, ...focused, ...guarded]).size, all.length);
  assert.deepEqual(guarded, ['test/integration/guard-runner.test.mjs']);
});

for (const defect of [
  'failed leg',
  'incomplete leg',
  'unsupported skip',
  'eligible applicability forgery',
  'C2 skip',
  'shared iframe skip',
  'false producer pass',
  'wrong corpus',
]) {
  test(`guard union rejects behavioural defect ${defect} even with complete keys`, () => {
    const input = documents();
    const row = input
      .flatMap((document) => document.report.results)
      .find((row) =>
        defect === 'shared iframe skip'
          ? row.id === 'C4.iframe-preattach.blank.shared'
          : defect === 'unsupported skip'
            ? row.id === 'C1.href'
            : row.id === 'C2.redirect-body'
      );
    if (defect === 'failed leg') row.guarded = 'fail';
    if (defect === 'incomplete leg') row.live = 'incomplete';
    if (
      [
        'unsupported skip',
        'eligible applicability forgery',
        'C2 skip',
        'shared iframe skip',
      ].includes(defect)
    ) {
      row.control = row.guarded = row.live = 'skip';
      row.reason =
        defect === 'eligible applicability forgery'
          ? 'not-applicable'
          : 'not-supported:browserSession';
    }
    if (defect === 'false producer pass') input[0].report.pass = false;
    if (defect === 'wrong corpus') input[0].report.set = 'unverified corpus';
    assert.throws(() => assertPartitionUnion(vectors, input, head, run));
  });
}

for (const defect of [
  'missing partition',
  'duplicate partition',
  'unknown partition',
  'wrong head',
  'wrong run',
  'wrong report head',
  'wrong selection',
  'missing row',
  'duplicate row',
  'extra row',
]) {
  test(`guard union rejects ${defect}`, () => {
    const input = documents();
    if (defect === 'missing partition') input.pop();
    if (defect === 'duplicate partition') input[3] = input[0];
    if (defect === 'unknown partition') input[0].id = 'invented';
    if (defect === 'wrong head') input[0].head = 'other';
    if (defect === 'wrong run') input[0].run = 'other';
    if (defect === 'wrong report head') input[0].report.sha = 'other';
    if (defect === 'wrong selection') input[0].report.selection.targets = [];
    if (defect === 'missing row') input[0].report.results.pop();
    if (defect === 'duplicate row') input[0].report.results.push(input[0].report.results[0]);
    if (defect === 'extra row')
      input[0].report.results.push({ id: 'invented', target: 'ordinary', variant: 'prompt' });
    assert.throws(() => assertPartitionUnion(vectors, input, head, run));
  });
}
