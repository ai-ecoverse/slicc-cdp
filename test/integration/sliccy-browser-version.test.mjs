import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import browser, { version } from '../../packages/sliccy-browser/src/browser.js';

test('sliccy:browser reports the version of the package it ships in', async () => {
  const manifest = JSON.parse(
    await readFile(new URL('../../packages/sliccy-browser/package.json', import.meta.url), 'utf8')
  );
  assert.equal(version, manifest.version);
  assert.equal(browser.version, manifest.version);
});
