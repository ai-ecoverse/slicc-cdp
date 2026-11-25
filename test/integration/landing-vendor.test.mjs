import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';
import { pathToFileURL } from 'node:url';
import vm from 'node:vm';

const file = path.resolve(import.meta.dirname, '../../crates/playwright-cli/src/landing.js');
const source = await readFile(file, 'utf8');

function landing(navigator) {
  class Element {}
  const context = vm.createContext({
    navigator,
    document: { visibilityState: 'visible', hasFocus: () => true },
    ShadowRoot: class extends Element {},
    HTMLIFrameElement: class extends Element {},
    HTMLFrameElement: class extends Element {},
    HTMLObjectElement: class extends Element {},
    HTMLEmbedElement: class extends Element {},
    HTMLTextAreaElement: class extends Element {},
    HTMLInputElement: class extends Element {},
    addEventListener() {},
    removeEventListener() {},
  });
  return vm.runInContext(source, context, { filename: pathToFileURL(file).href });
}

test('landing arms and reads visibility on a browser that reports no navigator.vendor', () => {
  assert.equal(landing({})('visible', {}), 'visible');
  assert.equal(landing({})('arm', { types: ['click'] }), 'armed');
  assert.equal(landing({ vendor: null })('visible', {}), 'visible');
});

test('landing still treats an Apple vendor as untrusted input and a Google one as trusted', () => {
  assert.equal(landing({ vendor: 'Apple Computer, Inc.' })('visible', {}), 'untrusted');
  assert.equal(landing({ vendor: 'Apple Computer, Inc.' })('arm', { types: [] }), 'untrusted');
  assert.equal(landing({ vendor: 'Google Inc.' })('visible', {}), 'visible');
});
