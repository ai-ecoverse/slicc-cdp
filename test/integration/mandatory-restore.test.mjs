import assert from 'node:assert/strict';
import test from 'node:test';
import {
  bfcachePrivacy,
  mandatory,
  protectedBfcacheRows,
  published,
  verdictProblems,
} from './guard-set.mjs';
import { syntheticReport } from './guard-test-evidence.mjs';

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

const base = 'protected';
const marker = `bfcache-marker-${base}-p`;
const ordinary = `bfcache-marker-${base}-b`;
const restored = (id = 'C8.protected-bfcache-restore') => ({
  ...clean(id),
  evidence: {
    guarded: {
      restore: {
        seed: {
          protectedURL: `https://vault.test:8443/bfcache?n=${encodeURIComponent(`${base}-p`)}`,
        },
      },
    },
  },
});
const scored = (messages, id) => {
  const row = restored(id);
  return { ...row, guardedPrivacy: bfcachePrivacy(row, messages) };
};
const carriers = [
  (value) => value,
  encodeURIComponent,
  (value) => Buffer.from(value).toString('base64'),
  (value) => JSON.stringify({ value }),
  (value) => ({ method: 'Runtime.consoleAPICalled', params: { args: [{ value }] } }),
];

test('the P marker followed by a word character is exposed, as the vector scores it', () => {
  for (const id of protectedBfcacheRows)
    for (const suffix of ['2', 'b', '_', 'x9'])
      for (const encode of carriers) {
        const row = scored([encode(`${marker}${suffix}`)], id);
        assert.equal(row.guardedPrivacy.markerExposed, true, `${id} ${suffix}`);
        assert(verdictProblems([row]).length > 0, `${id} ${suffix}`);
      }
});

test('the exact P marker is exposed in every carrier', () => {
  for (const id of protectedBfcacheRows)
    for (const encode of carriers) {
      const row = scored([encode(marker)], id);
      assert.equal(row.guardedPrivacy.markerExposed, true, id);
      assert(verdictProblems([row]).length > 0, id);
    }
});

test('the ordinary B marker and its path never count as P exposure', () => {
  for (const id of protectedBfcacheRows)
    for (const encode of carriers) {
      const row = scored(
        [encode(ordinary), encode(`https://ordinary.test/bfcache?n=${base}-b`)],
        id
      );
      assert.equal(row.guardedPrivacy.markerExposed, false, id);
      assert.equal(row.guardedPrivacy.protectedPathExposed, false, id);
      assert.deepEqual(verdictProblems([row]), [], id);
    }
});

test('the protected path and query count as exposure by substring', () => {
  for (const value of [
    `vault.test:8443/bfcache?x`,
    `https://vault.test:8443/bfcache`,
    `/bfcache?n=${base}-p2`,
  ]) {
    const row = scored([value]);
    assert.equal(row.guardedPrivacy.protectedPathExposed, true, value);
    assert(verdictProblems([row]).length > 0, value);
  }
});

test('scoring records only measurement and exposure, with no local audit fields', () => {
  assert.deepEqual(Object.keys(scored([ordinary]).guardedPrivacy).sort(), [
    'markerExposed',
    'measured',
    'protectedPathExposed',
  ]);
  assert.deepEqual(bfcachePrivacy(restored(), []), { measured: false });
  assert.deepEqual(bfcachePrivacy(clean('C8.protected-bfcache-restore'), [marker]), {
    measured: false,
  });
});

test('the synthetic report still scores clean restore rows without problems', () => {
  const rows = syntheticReport(published).results.filter((row) => protectedBfcacheRows.has(row.id));
  assert(rows.length > 0);
  assert.deepEqual(verdictProblems(rows), []);
});
