import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { protectedOrigins } from '../../packages/sliccy-browser/src/protected.js';
import { PROTECTED_ORIGINS } from '../../packages/sliccy-browser/src/protected-origins.js';

const load = async (name) =>
  JSON.parse(
    await readFile(
      fileURLToPath(import.meta.resolve(`@ai-ecoverse/slicc-browsers/${name}`)),
      'utf8'
    )
  );
const catalog = await load('catalog.json');
const vectors = await load('protected.json');
const vendored = protectedOrigins(PROTECTED_ORIGINS, vectors.origins);
const reference = protectedOrigins(catalog.protectedOrigins, vectors.origins);

const rows = [
  ['AbOuT:blank', true],
  ['about:', true],
  ['abouT:', true],
  ['about:\n', true],
  ['about://\n', true],
  ['about://\r', true],
  ['about://\t', true],
  ['about:blank ', true],
  ['about://x\u0000', true],
  ['about:%20', true],
  ['ABOUT://', true],
  ['', false],
  ['null', false],
  ['://', false],
  ['about://', false],
];

rows.forEach(([contextOrigin, want], index) => {
  const name = `row ${String(index + 1).padStart(2, '0')} ${want ? 'protected' : 'unprotected'}`;
  test(name, () => {
    const matching = vectors.targets.filter(
      (vector) =>
        Object.keys(vector).length === 3 &&
        vector.url === 'about:blank' &&
        vector.contextOrigin === contextOrigin
    );
    assert.equal(matching.length, 1, JSON.stringify(contextOrigin));
    assert.equal(matching[0].protected, want, JSON.stringify(contextOrigin));
    const entry = { url: 'about:blank', contextOrigin };
    assert.equal(vendored.target(entry), want, JSON.stringify(contextOrigin));
    assert.equal(vendored.target(matching[0]), want, JSON.stringify(contextOrigin));
    assert.equal(reference.target(entry), want, JSON.stringify(contextOrigin));
  });
});
