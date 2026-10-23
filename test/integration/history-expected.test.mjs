import assert from 'node:assert/strict';
import test from 'node:test';
import { summary } from '@ai-ecoverse/slicc-browsers/guard-runner';
import * as guard from './guard-set.mjs';
import { syntheticReport } from './guard-test-evidence.mjs';

const {
  bfcachePrivacy,
  containsMarkerToken,
  historyRepliesProvenOrdinaryB,
  isExpectedBfcacheFailure477,
  isExpectedBfcacheHistoryFailure477,
  published,
  validApplicabilitySkip,
  verdictProblems,
} = guard;

const tag = 'public-history-regression';
const marker = `bfcache-marker-${tag}`;
const ordinary = `http://plain.guard.test:12345/bfcache?n=${tag}-b`;
const protectedURL = `http://seven.sliccy.ai:12345/bfcache?n=${tag}`;

function row() {
  const state = {
    origin: new URL(ordinary).origin,
    marker: `${marker}-b`,
    stored: `${marker}-b`,
    persisted: false,
  };
  const reply = (method, result) => ({
    method,
    protectedCurrent: false,
    outcome: 'answered',
    reply: { ok: true, result },
  });
  const restore = {
    seed: { protectedURL, ordinaryAnswer: { result: { frameTree: { frame: { url: ordinary } } } } },
    trigger: { ok: false, error: { message: 'no answer' } },
    delay: {
      acknowledged: true,
      seeding: { protectedObserved: true, ordinaryObserved: true },
      release: { ok: true, result: { delayedEvents: 0 } },
    },
    beforeRelease: { ...state },
    sideEffect: { ...state },
    screenshotProof: {
      url: ordinary,
      reply: { ok: true, result: { data: 'ORDINARY_B_PNG_BYTES' } },
    },
    probes: [
      reply('Runtime.evaluate', { result: { value: JSON.stringify(state) } }),
      reply('Runtime.callFunctionOn', { result: { value: JSON.stringify(state) } }),
      reply('DOM.getDocument', { root: { documentURL: ordinary } }),
      reply('Accessibility.getFullAXTree', { nodes: [{ name: { value: `${marker}-b` } }] }),
      reply('Page.captureScreenshot', { data: 'ORDINARY_B_PNG_BYTES' }),
      reply('Input.dispatchKeyEvent', {}),
    ],
  };
  const result = {
    id: 'C8.protected-bfcache-restore.history',
    target: 'seven',
    variant: 'prompt',
    control: 'pass',
    guarded: 'fail',
    live: 'pass',
    observations: {
      guarded: {
        markerHidden: false,
        originHidden: true,
        restoredObserved: 'incomplete',
        sideEffectAbsent: 'incomplete',
        probesRefusedOrUnforwarded: false,
        sessionLive: true,
      },
    },
    evidence: { guarded: { restore } },
  };
  result.guardedPrivacy = bfcachePrivacy(
    result,
    restore.probes.map((probe) => JSON.stringify(probe.reply))
  );
  return result;
}

const encoded = [
  (value) => value,
  (value) => value.replace(/-b$/, '%2Db'),
  (value) => value.replace(/-b$/, '%252Db'),
  (value) => value.replace(/-b$/, '\\u002db'),
  (value) => encodeURIComponent(value),
  (value) =>
    [...Buffer.from(value)].map((byte) => `%${byte.toString(16).padStart(2, '0')}`).join(''),
  (value) =>
    [...value].map((char) => `\\u${char.charCodeAt(0).toString(16).padStart(4, '0')}`).join(''),
  (value) => Buffer.from(value).toString('base64'),
  (value) => Buffer.from(JSON.stringify({ secret: value })).toString('base64url'),
];

test('history no-answer expected class requires ordinary B bookends, real replies and independent matching screenshot proof', () => {
  const result = row();
  const raw = structuredClone(result.observations);
  assert.equal(isExpectedBfcacheFailure477(result), true);
  assert.equal(isExpectedBfcacheHistoryFailure477(result), true);
  assert.deepEqual(verdictProblems([result]), []);
  assert.equal(result.guarded, 'fail');
  assert.deepEqual(result.observations, raw);
  assert.equal(result.guardedPrivacy.upstreamMarkerSubstring, true);
  assert.equal(result.guardedPrivacy.markerExposed, false);
  assert.match(result.guardedPrivacy.auditDifference, /public ordinary-B/);
  for (const target of ['seven', 'vault'])
    for (const variant of ['prompt', 'lagging'])
      assert.equal(isExpectedBfcacheHistoryFailure477({ ...result, target, variant }), true);
});

test('whole-token privacy audit rejects P marker and path in raw and encoded replies without mistaking the public B marker', () => {
  for (const encode of encoded) {
    assert.equal(containsMarkerToken(encode(marker), marker), true);
    assert.equal(containsMarkerToken(encode(`${marker}-b`), marker), false);
    for (const secret of [
      marker,
      protectedURL,
      `${new URL(protectedURL).pathname}${new URL(protectedURL).search}`,
    ]) {
      const result = row();
      result.guardedPrivacy = bfcachePrivacy(result, [encode(`${marker}-b`), encode(secret)]);
      assert.equal(isExpectedBfcacheFailure477(result), false);
      assert.ok(verdictProblems([result]).length > 0);
    }
  }
});

test('history exception rejects any missing, mismatched, protected or unknown evidence', () => {
  const changes = [
    (r) => {
      r.id = 'C8.protected-bfcache-restore';
    },
    (r) => {
      r.target = 'ordinary';
    },
    (r) => {
      r.variant = 'invented';
    },
    (r) => {
      r.control = 'fail';
    },
    (r) => {
      r.guarded = 'incomplete';
    },
    (r) => {
      r.live = 'fail';
    },
    (r) => {
      delete r.guardedPrivacy;
    },
    (r) => {
      r.guardedPrivacy.markerIdentity = 'substring';
    },
    (r) => {
      delete r.guardedPrivacy.auditDifference;
    },
    (r) => {
      r.guardedPrivacy.ordinaryBMarkerObserved = false;
    },
    (r) => {
      r.evidence.guarded.restore.trigger.error.message = 'refused';
    },
    (r) => {
      r.evidence.guarded.restore.delay.release.result.delayedEvents = 1;
    },
    (r) => {
      r.evidence.guarded.restore.restoreAt = 1;
    },
    (r) => {
      r.evidence.guarded.restore.beforeRelease.persisted = true;
    },
    (r) => {
      r.evidence.guarded.restore.sideEffect.origin = new URL(protectedURL).origin;
    },
    (r) => {
      delete r.evidence.guarded.restore.beforeRelease;
    },
    (r) => {
      r.evidence.guarded.restore.probes[4].protectedCurrent = true;
    },
    (r) => {
      delete r.evidence.guarded.restore.probes[4].protectedCurrent;
    },
    (r) => {
      r.evidence.guarded.restore.screenshotProof.url = protectedURL;
    },
    (r) => {
      delete r.evidence.guarded.restore.screenshotProof;
    },
    (r) => {
      r.evidence.guarded.restore.probes[4].reply.result.data = 'PROTECTED_P_SCREENSHOT';
    },
    (r) => {
      r.evidence.guarded.restore.probes[0].reply.result.result.value = JSON.stringify({
        origin: new URL(protectedURL).origin,
        marker,
      });
    },
    (r) => {
      r.evidence.guarded.restore.probes[2].reply.result.root.documentURL = protectedURL;
    },
    (r) => {
      r.evidence.guarded.restore.probes[3].reply.result.nodes = [{ name: { value: marker } }];
    },
    (r) => {
      r.evidence.guarded.restore.probes.pop();
      r.evidence.guarded.restore.probes[0].reply.result.result.value = 'unknown';
    },
  ];
  for (const change of changes) {
    const result = row();
    change(result);
    assert.equal(isExpectedBfcacheHistoryFailure477(result), false);
    assert.equal(isExpectedBfcacheFailure477(result), false);
    assert.ok(verdictProblems([result]).length > 0);
  }
});

test('canonical origin-unavailable and declared reasons are accepted only by the published validator', () => {
  for (const id of ['C4.static-same-site', 'C4.script-same-site']) {
    const vector = published.vectors.find((vector) => vector.id === id);
    for (const reason of [
      'not-applicable:no-ordinary-same-site-origin',
      'not-applicable: every sliccy.ai host is protected, so no unprotected page is same-site with the vault',
    ]) {
      const result = {
        id,
        target: 'vault',
        variant: 'prompt',
        control: 'skip',
        guarded: 'skip',
        live: 'skip',
        reason,
      };
      assert.equal(validApplicabilitySkip(vector, result), true);
      assert.deepEqual(verdictProblems([result]), []);
      assert.equal(result.reason, reason);
    }
    assert.equal(
      validApplicabilitySkip(vector, {
        id,
        target: 'vault',
        variant: 'prompt',
        reason: 'not-applicable:invented',
      }),
      false
    );
  }
  const targets = guard.partitions.b;
  const set = {
    ...published,
    vectors: published.vectors
      .map((vector) => ({
        ...vector,
        targets: vector.targets.filter((target) => targets.includes(target)),
      }))
      .filter((vector) => vector.targets.length),
  };
  const report = syntheticReport(set, 'public-vault-origin-roundtrip');
  const rows = report.results.filter(
    (row) =>
      row.target === 'vault' && ['C4.static-same-site', 'C4.script-same-site'].includes(row.id)
  );
  assert.equal(rows.length, 4);
  for (const row of rows) row.reason = 'not-applicable:no-ordinary-same-site-origin';
  const roundtrip = JSON.parse(
    JSON.stringify({
      ...summary({ set, results: report.results, guard: 'slicc-cdp', sha: report.sha }),
      selection: { targets },
    })
  );
  guard.assertGuardReport(roundtrip, 'b', report.sha);
  assert.equal(
    roundtrip.results
      .filter((row) => rows.some((original) => original.id === row.id && row.target === 'vault'))
      .every((row) => row.reason === 'not-applicable:no-ordinary-same-site-origin'),
    true
  );
  assert.equal(historyRepliesProvenOrdinaryB({}), false);
});
