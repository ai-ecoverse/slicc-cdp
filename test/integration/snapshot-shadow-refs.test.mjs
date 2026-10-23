import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { runInNewContext } from 'node:vm';

const file = fileURLToPath(
  new URL('../../crates/playwright-cli/src/aria-snapshot.js', import.meta.url)
);
const source = readFileSync(file, 'utf8');
const start = source.indexOf('function cssPath(');
const end = source.lastIndexOf('return sliccAriaSnapshot;');

function capture(prefix) {
  const html = { children: [] };
  const ownerDocument = { documentElement: html };
  html.ownerDocument = ownerDocument;
  const body = { ownerDocument, parentElement: html, children: [] };
  html.children.push(body);
  const host = { ownerDocument, parentElement: body, children: [] };
  const light = { ownerDocument, parentElement: body };
  body.children.push(host, light);
  const shadow = { ownerDocument, parentElement: null, parentNode: { host } };
  const json = [
    { role: 'button', name: 'Shadow control', ref: `${prefix}e1` },
    { role: 'button', name: 'Light control', ref: `${prefix}e2` },
  ];
  const sandbox = {
    document: { querySelector: () => body, baseURI: 'https://example.test/' },
    generateAriaTree: () => ({
      info: new Map([
        [`${prefix}e1`, { element: shadow }],
        [`${prefix}e2`, { element: light }],
      ]),
    }),
    renderAriaTreeAsJSON: () => ({ json, iframeDepths: {} }),
    wellFormed: (value) => value,
  };
  const run = runInNewContext(`${source.slice(start, end)}\nsliccAriaSnapshot;`, sandbox, {
    filename: file,
    lineOffset: source.slice(0, start).split('\n').length - 1,
  });
  return { result: run({ prefix, key: 'elements' }), sandbox };
}

test('iframe snapshots do not emit refs with unresolvable shadow selectors', () => {
  const { result, sandbox } = capture('f1');
  assert.equal(result.json[0].name, 'Shadow control');
  assert.equal(result.json[0].ref, undefined);
  assert.equal(result.refs.length, 1);
  assert.equal(result.refs[0].ref, 'f1e2');
  assert.equal(result.refs[0].selector, ':root > :nth-child(1) > :nth-child(2)');
  assert.equal(sandbox.elements.length, 1);
});

test('main-frame shadow refs remain available through backend node resolution', () => {
  const { result, sandbox } = capture('');
  assert.equal(result.json[0].ref, 'e1');
  assert.equal(result.refs[0].selector, '');
  assert.equal(result.refs.length, 2);
  assert.equal(sandbox.elements.length, 2);
});
