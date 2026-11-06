import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import vm from 'node:vm';

const chrome = await readFile(
  new URL('./webmcp-polyfill-chrome.test.mjs', import.meta.url),
  'utf8'
);
const wire = await readFile(new URL('./webmcp-wire.mjs', import.meta.url), 'utf8');
const start = chrome.indexOf(': `if (document.modelContext) {');
assert.ok(start >= 0);
const scriptStart = start + 3;
const end = chrome.indexOf('}`;', scriptStart);
assert.ok(end > scriptStart);
const names = Array.from({ length: 8 }, (_, index) => `tool-${index}`);
const script = chrome
  .slice(scriptStart, end + 1)
  .replace('${JSON.stringify(names)}', JSON.stringify(names))
  .replace('${JSON.stringify(ordinary)}', JSON.stringify('https://ordinary.invalid'));

for (const available of [true, false])
  test(`root fixture preserves conditional registration without a diagnostic tap (${available})`, () => {
    const registered = [];
    const context = vm.createContext({
      document: {
        modelContext: available
          ? { registerTool: (tool) => registered.push(tool.name) }
          : undefined,
      },
    });
    vm.runInContext(script, context);
    assert.deepEqual(registered, available ? names : []);
    assert.doesNotMatch(script, /setTimeout|setInterval|await|while\s*\(/);
  });

test('root tool count, raw endpoint and real conversion witness remain while diagnostics are removed', () => {
  assert.match(chrome, /assert\.equal\(JSON\.parse\(listed\.stdout\)\.length, 8\)/);
  assert.match(chrome, /const listed = await once\(browser\.wsUrl/);
  assert.match(chrome, /holder = command\(browser\.wsUrl, cwd/);
  assert.match(chrome, /conversionHolder = command\(wire\.url, cwd/);
  assert.doesNotMatch(chrome, /cdp675Registration|webmcp-root-tools-tap|mainPolicy/);
  assert.doesNotMatch(wire, /responses|permissions|probes|policyObserved|toolsPresent|location:/);
  assert.match(wire, /send\(value\)/);
  assert.match(wire, /peer\.ws\.send\(JSON\.stringify\(message\)\)/);
});
