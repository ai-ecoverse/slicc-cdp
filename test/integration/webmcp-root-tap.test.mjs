import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import vm from 'node:vm';

const chrome = await readFile(
  new URL('./webmcp-polyfill-chrome.test.mjs', import.meta.url),
  'utf8'
);
const wire = await readFile(new URL('./webmcp-wire.mjs', import.meta.url), 'utf8');
const start = chrome.indexOf('if (document.modelContext) {');
const end = chrome.indexOf('\n</script>', start);
assert.ok(start >= 0 && end > start);
const names = Array.from({ length: 8 }, (_, index) => `tool-${index}`);
const script = chrome
  .slice(start, end)
  .replace('${JSON.stringify(names)}', JSON.stringify(names))
  .replace('${JSON.stringify(ordinary)}', JSON.stringify('https://ordinary.invalid'));

for (const available of [true, false])
  test(`CI-only root registration tap records the original conditional outcome (${available})`, () => {
    const registered = [];
    const context = vm.createContext({
      document: {
        modelContext: available
          ? { registerTool: (tool) => registered.push(tool.name) }
          : undefined,
      },
    });
    vm.runInContext(script, context);
    assert.equal(context.cdp675Registration.branch, available);
    assert.equal(context.cdp675Registration.attempted, available ? 8 : 0);
    assert.equal(context.cdp675Registration.registered, available ? 8 : 0);
    assert.deepEqual(registered, available ? names : []);
    assert.doesNotMatch(script, /setTimeout|setInterval|await|while\s*\(/);
  });

test('root tool count remains eight while the passive trace names normal instruction location and listing policy', () => {
  assert.match(chrome, /assert\.equal\(JSON\.parse\(listed\.stdout\)\.length, 8\)/);
  assert.match(chrome, /const listed = await once\(wire\.url/);
  assert.match(chrome, /webmcp-root-tools-tap/);
  assert.match(wire, /location: params\.callFrames\?\.\[0\]\?\.location/);
  assert.match(wire, /command\.method === 'Page\.getPermissionsPolicyState'/);
  assert.match(wire, /toolsPresent: !!tools,\s*allowed: tools\?\.allowed/);
  assert.match(wire, /policy:\s*Object\.entries\(headers\)/);
  assert.match(wire, /send\(value\)/);
  assert.match(wire, /peer\.ws\.send\(JSON\.stringify\(message\)\)/);
});
