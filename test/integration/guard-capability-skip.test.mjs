import assert from 'node:assert/strict';
import test from 'node:test';
import {
  declaredCapabilityNames,
  mandatory,
  protectedBfcacheRows,
  published,
  requiredCapabilities,
  runCapabilities,
  validCapabilitySkip,
  verdictProblems,
} from './guard-set.mjs';

const stock = published.vectors.find(
  (vector) =>
    Array.isArray(vector.requires) &&
    vector.requires.includes('stockSetup') &&
    !mandatory(vector.id)
);
assert.ok(stock, '1.21.0 publishes a stockSetup-gated vector');

const plain = published.vectors.find(
  (vector) =>
    vector.kind !== 'text' &&
    !vector.skip &&
    !vector.observable &&
    !mandatory(vector.id) &&
    !(vector.requires ?? []).includes('stockSetup') &&
    !vector.id.startsWith('C8.protected-bfcache-restore')
);
assert.ok(plain);

const row = (vector, target, legs, reason) => ({
  id: vector.id,
  target,
  variant: 'prompt',
  control: legs,
  guarded: legs,
  live: legs,
  ...(reason === undefined ? {} : { reason }),
});

test('an undeclared required-capability skip is accepted', () => {
  const target = stock.targets[0];
  const skipped = row(stock, target, 'skip', 'not-supported:stockSetup');
  assert.equal(validCapabilitySkip(stock, skipped), true);
  assert.deepEqual(verdictProblems([skipped]), []);
});

test('a not-supported skip for a capability the vector does not require is rejected', () => {
  const target = plain.targets[0];
  const skipped = row(plain, target, 'skip', 'not-supported:stockSetup');
  assert.equal(validCapabilitySkip(plain, skipped), false);
  assert.equal(verdictProblems([skipped]).length, 1);
});

test('a not-supported skip with a non-capability reason stays rejected', () => {
  const target = stock.targets[0];
  for (const reason of [
    'not-supported:unguarded-observer',
    'not-supported:trustedEmulation',
    'not-supported:clientExit',
    'not-applicable:no-ordinary-same-site-origin',
    'not-supported:',
    'not-supported:stockSetup:extra',
  ]) {
    assert.equal(validCapabilitySkip(stock, row(stock, target, 'skip', reason)), false, reason);
    assert.equal(verdictProblems([row(stock, target, 'skip', reason)]).length, 1, reason);
  }
});

test('a declared capability that skipped is rejected', () => {
  const target = stock.targets[0];
  const withStock = { ...runCapabilities, stockSetup: true };
  const skipped = row(stock, target, 'skip', 'not-supported:stockSetup');
  assert.equal(validCapabilitySkip(stock, skipped, withStock), false);
  assert.equal(
    verdictProblems([skipped]).length,
    0,
    'the live run does not declare stockSetup so the row stays accepted there'
  );
  const browser = published.vectors.find(
    (vector) =>
      Array.isArray(vector.requires) &&
      vector.requires.includes('browserSession') &&
      !vector.requires.includes('stockSetup') &&
      !mandatory(vector.id)
  );
  assert.ok(browser);
  const declaredSkip = row(browser, browser.targets[0], 'skip', 'not-supported:browserSession');
  assert.equal(validCapabilitySkip(browser, declaredSkip), false);
  assert.equal(verdictProblems([declaredSkip]).length, 1);
});

test('a mandatory row that skipped is never accepted', () => {
  const id = [...published.vectors.map((vector) => vector.id)].find((name) => mandatory(name));
  assert.ok(id);
  const vector = published.vectors.find((entry) => entry.id === id);
  const skipped = row(vector, vector.targets[0], 'skip', 'not-supported:stockSetup');
  assert.equal(verdictProblems([skipped]).length, 1);
});

test('the signInHold skip is accepted only for the sign-in hold vectors', () => {
  const holds = published.vectors.filter((vector) =>
    requiredCapabilities(vector).has('signInHold')
  );
  assert.ok(holds.length > 0, 'the published set has sign-in hold vectors');
  for (const vector of holds) {
    assert.match(vector.id, /^C7\.sign-in-hold\./);
    const skipped = row(vector, vector.targets[0], 'skip', 'not-supported:signInHold');
    assert.equal(validCapabilitySkip(vector, skipped), true, vector.id);
  }
  assert.equal(runCapabilities.signInHold, undefined);
  const skipped = row(plain, plain.targets[0], 'skip', 'not-supported:signInHold');
  assert.equal(validCapabilitySkip(plain, skipped), false);
  assert.notDeepEqual(verdictProblems([skipped]), []);
});

test('the run does not declare autoAttach because the route refuses auto-attach', () => {
  assert.equal(declaredCapabilityNames(runCapabilities).has('autoAttach'), false);
  assert.notEqual(runCapabilities.autoAttach, true);
});

test('the run declares Fetch interception and not legacy Network interception', () => {
  const declared = declaredCapabilityNames(runCapabilities);
  assert.equal(declared.has('requestInterception'), true);
  assert.equal(declared.has('fetchInterception'), true);
  assert.equal(declared.has('legacyInterception'), false);
  assert.notEqual(runCapabilities.legacyInterception, true);
});

test('a not-supported:legacyInterception skip is accepted only when the vector requires it', () => {
  const plant = published.vectors.find((vector) => vector.id === 'C10.cookie-plant');
  assert.ok(plant);
  assert.equal(requiredCapabilities(plant).has('requestInterception'), true);
  const legacyOnly = published.vectors.find(
    (vector) =>
      requiredCapabilities(vector).has('legacyInterception') &&
      !requiredCapabilities(vector).has('fetchInterception') &&
      !mandatory(vector.id)
  );
  if (legacyOnly) {
    const skipped = row(
      legacyOnly,
      legacyOnly.targets[0],
      'skip',
      'not-supported:legacyInterception'
    );
    assert.equal(validCapabilitySkip(legacyOnly, skipped), true);
    assert.deepEqual(verdictProblems([skipped]), []);
  }
  const skipped = row(plain, plain.targets[0], 'skip', 'not-supported:legacyInterception');
  assert.equal(validCapabilitySkip(plain, skipped), false);
  assert.equal(verdictProblems([skipped]).length, 1);
});

test('a not-supported:autoAttach skip is accepted only for vectors that require autoAttach', () => {
  const gated = [
    'C3.popup',
    'C3.popup-self-nav',
    'C3.popup-on-redirect-302',
    'C3.popup-on-redirect-chain',
    'C3.direct-popup-stays-out',
    'C3.redirect-popup-stays-out',
    'C4.static-cross-site',
    'C4.script-cross-site',
  ];
  for (const id of gated) {
    const vector = published.vectors.find((entry) => entry.id === id);
    assert.ok(vector, id);
    assert.ok(vector.requires.includes('autoAttach'), id);
    assert.equal(mandatory(id), false, id);
    for (const target of vector.targets) {
      const skipped = row(vector, target, 'skip', 'not-supported:autoAttach');
      assert.equal(validCapabilitySkip(vector, skipped), true, `${id} ${target}`);
      assert.deepEqual(verdictProblems([skipped]), [], `${id} ${target}`);
    }
  }
  const frame = published.vectors.find((entry) => entry.id === 'C9.frame-hidden');
  assert.ok(frame);
  assert.equal(requiredCapabilities(frame).has('autoAttach'), true);
  const other = published.vectors.find(
    (vector) =>
      vector.kind !== 'text' &&
      !mandatory(vector.id) &&
      !requiredCapabilities(vector).has('autoAttach')
  );
  assert.ok(other);
  const skipped = row(other, other.targets[0], 'skip', 'not-supported:autoAttach');
  assert.equal(validCapabilitySkip(other, skipped), false);
  assert.equal(verdictProblems([skipped]).length, 1);
});

test('no mandatory row requires autoAttach or can take its skip', () => {
  const rows = published.vectors.filter((vector) => mandatory(vector.id));
  const ids = rows.map((vector) => vector.id);
  for (const id of protectedBfcacheRows) assert.ok(ids.includes(id), id);
  assert.ok(ids.includes('X1.after-exit'));
  assert.ok(ids.some((id) => id.startsWith('C2.redirect-')));
  assert.ok(ids.some((id) => /^C4\.iframe-preattach\..*\.shared$/.test(id)));
  for (const vector of rows) {
    assert.equal(requiredCapabilities(vector).has('autoAttach'), false, vector.id);
    for (const target of vector.targets) {
      const skipped = row(vector, target, 'skip', 'not-supported:autoAttach');
      assert.equal(validCapabilitySkip(vector, skipped), false, `${vector.id} ${target}`);
      assert.equal(verdictProblems([skipped]).length, 1, `${vector.id} ${target}`);
    }
  }
});
