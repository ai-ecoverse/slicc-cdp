import assert from 'node:assert/strict';
import test from 'node:test';
import { summary } from '@ai-ecoverse/slicc-browsers/guard-runner';
import {
  assertGuardReport,
  bfcachePrivacy,
  expectedBfcacheFailure477,
  expectedBfcacheFailureReason,
  isExpectedBfcacheFailure477,
  partitions,
  published,
  verdictProblems,
} from './guard-set.mjs';
import { syntheticReport } from './guard-test-evidence.mjs';

function fixture(id = 'C8.protected-bfcache-restore') {
  const targets = partitions.a;
  const set = {
    ...published,
    vectors: published.vectors
      .map((vector) => ({
        ...vector,
        targets: vector.targets.filter((target) => targets.includes(target)),
      }))
      .filter((vector) => vector.targets.length),
  };
  const report = syntheticReport(set, 'expected-fixture-head');
  const row = report.results.find((row) => row.id === id);
  row.guarded = 'fail';
  row.observations = {
    guarded: {
      markerHidden: true,
      originHidden: true,
      restoredObserved: true,
      sessionLive: true,
      sideEffectAbsent: false,
      probesRefusedOrUnforwarded: false,
    },
  };
  row.evidence = {
    guarded: {
      restore: {
        seed: { protectedURL: 'https://seven.sliccy.ai/bfcache?n=synthetic-tag' },
        probes: [],
      },
    },
  };
  row.guardedPrivacy = bfcachePrivacy(row, ['{"result":{"url":"https://seven.sliccy.ai"}}']);
  return {
    row,
    report: {
      ...summary({ set, results: report.results, guard: 'slicc-cdp', sha: report.sha }),
      selection: { targets },
    },
  };
}

test('only the two isolated #477 admission failures are expected, with raw FAILs retained', () => {
  assert.equal(expectedBfcacheFailureReason, 'slicc-cdp#477');
  assert.deepEqual([...expectedBfcacheFailure477].sort(), [
    'C8.protected-bfcache-restore',
    'C8.protected-bfcache-restore.history',
  ]);
  for (const id of expectedBfcacheFailure477) {
    const { row, report } = fixture(id);
    assert.equal(report.pass, false);
    assert.equal(row.guarded, 'fail');
    assert.equal(isExpectedBfcacheFailure477(row), true);
    assert.deepEqual(verdictProblems([row]), []);
    assertGuardReport(report, 'a', 'expected-fixture-head');
    assert.equal(row.guarded, 'fail');
  }
});

for (const defect of [
  'marker',
  'path',
  'guarded observation leak',
  'origin observation leak',
  'live failure',
  'control failure',
  'missing privacy',
  'unrelated failure',
  'screenshot disclosure',
  'skipped row',
  'incomplete row',
]) {
  test(`the #477 exception rejects ${defect}`, () => {
    const { row, report } = fixture();
    if (defect === 'marker')
      row.guardedPrivacy = bfcachePrivacy(row, ['{"live":"bfcache-marker-synthetic-tag"}']);
    if (defect === 'path')
      row.guardedPrivacy = bfcachePrivacy(row, [
        JSON.stringify({
          delivered: Buffer.from('https://seven.sliccy.ai/bfcache?n=synthetic-tag').toString(
            'base64'
          ),
        }),
      ]);
    if (defect === 'guarded observation leak') row.observations.guarded.markerHidden = false;
    if (defect === 'origin observation leak') row.observations.guarded.originHidden = false;
    if (defect === 'live failure') row.live = 'fail';
    if (defect === 'control failure') row.control = 'fail';
    if (defect === 'missing privacy') delete row.guardedPrivacy;
    if (defect === 'unrelated failure')
      report.results.find((row) => row.id === 'C2.redirect-secret').guarded = 'fail';
    if (defect === 'screenshot disclosure')
      row.evidence.guarded.restore.probes = [
        {
          protectedCurrent: true,
          method: 'Page.captureScreenshot',
          reply: { ok: true, result: { data: 'protected pixels' } },
        },
      ];
    if (defect === 'skipped row') row.control = row.guarded = row.live = 'skip';
    if (defect === 'incomplete row') row.guarded = 'incomplete';
    assert.throws(() => assertGuardReport(report, 'a', 'expected-fixture-head'));
  });
}

test('PASS labels never conceal missing privacy evidence or delivered protected content', () => {
  for (const defect of [
    'path',
    'marker observation',
    'origin observation',
    'missing audit',
    'screenshot',
  ]) {
    const { row } = fixture();
    row.control = row.guarded = row.live = 'pass';
    if (defect === 'path')
      row.guardedPrivacy = bfcachePrivacy(row, ['https://seven.sliccy.ai/bfcache?n=synthetic-tag']);
    if (defect === 'marker observation') row.observations.guarded.markerHidden = false;
    if (defect === 'origin observation') row.observations.guarded.originHidden = false;
    if (defect === 'missing audit') delete row.guardedPrivacy;
    if (defect === 'screenshot')
      row.evidence.guarded.restore.probes = [
        {
          protectedCurrent: true,
          method: 'Page.captureScreenshot',
          reply: { ok: true, result: { data: 'protected pixels' } },
        },
      ];
    assert.equal(verdictProblems([row]).length, 1, defect);
  }
});
