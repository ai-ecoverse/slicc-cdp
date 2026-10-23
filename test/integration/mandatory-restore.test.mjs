import assert from 'node:assert/strict';
import test from 'node:test';
import {
  containsMarkerToken,
  mandatory,
  protectedBfcacheRows,
  verdictProblems,
} from './guard-set.mjs';

const clean = (id) => ({
  id,
  target: 'seven',
  variant: 'prompt',
  control: 'pass',
  guarded: 'pass',
  live: 'pass',
  guardedPrivacy: { measured: true, markerExposed: false, protectedPathExposed: false },
  x1Privacy: {
    measured: true,
    markerExposed: false,
    protectedExposed: false,
    screenshotExposed: false,
  },
  guardedForwardingAudit: { id: 'history-trigger', complete: true, forwarded: false },
});

test('protected restores and client exit require raw PASS in every leg and variant', () => {
  assert.deepEqual(
    [...protectedBfcacheRows],
    ['C8.protected-bfcache-restore', 'C8.protected-bfcache-restore.history']
  );
  for (const id of [...protectedBfcacheRows, 'X1.after-exit']) {
    assert(mandatory(id));
    for (const target of ['seven', 'vault'])
      for (const variant of ['prompt', 'lagging']) {
        const row = { ...clean(id), target, variant };
        assert.deepEqual(verdictProblems([row]), []);
        for (const leg of ['control', 'guarded', 'live'])
          for (const verdict of ['fail', 'skip', 'not-run', 'incomplete'])
            assert(
              verdictProblems([{ ...row, [leg]: verdict, reason: 'not-applicable' }]).length > 0
            );
      }
  }
});

test('history trigger requires terminal actual non-forwarding evidence, never missing or positive forwarding', () => {
  for (const audit of [
    undefined,
    {},
    { complete: false, forwarded: false },
    { complete: true },
    { complete: true, forwarded: true },
  ])
    assert(
      verdictProblems([
        { ...clean('C8.protected-bfcache-restore.history'), guardedForwardingAudit: audit },
      ]).length > 0
    );
});

test('raw PASS cannot excuse missing privacy measurement or any protected content', () => {
  for (const id of protectedBfcacheRows) {
    for (const privacy of [
      undefined,
      { measured: false },
      { measured: true, markerExposed: true, protectedPathExposed: false },
      { measured: true, markerExposed: false, protectedPathExposed: true },
    ])
      assert(verdictProblems([{ ...clean(id), guardedPrivacy: privacy }]).length > 0);
    assert(
      verdictProblems([
        {
          ...clean(id),
          evidence: {
            guarded: {
              restore: {
                probes: [
                  {
                    method: 'Page.captureScreenshot',
                    protectedCurrent: true,
                    reply: { ok: true, result: { data: 'private-image' } },
                  },
                ],
              },
            },
          },
        },
      ]).length > 0
    );
  }
  for (const field of ['markerExposed', 'protectedExposed', 'screenshotExposed'])
    assert(
      verdictProblems([
        {
          ...clean('X1.after-exit'),
          x1Privacy: { ...clean('X1.after-exit').x1Privacy, [field]: true },
        },
      ]).length > 0
    );
});

test('marker identity is whole-token and survives encoded carriers without confusing ordinary B', () => {
  const marker = 'bfcache-marker-protected-p';
  for (const encode of [
    (value) => value,
    encodeURIComponent,
    (value) => Buffer.from(value).toString('base64'),
    (value) => JSON.stringify({ value }),
  ]) {
    assert(containsMarkerToken(encode(marker), marker));
    assert(!containsMarkerToken(encode(`${marker}-b`), marker));
  }
});
