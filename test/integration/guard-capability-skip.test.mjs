import assert from 'node:assert/strict';
import test from 'node:test';
import {
  mandatory,
  published,
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
