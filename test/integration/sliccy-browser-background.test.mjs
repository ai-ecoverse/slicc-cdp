import assert from 'node:assert/strict';
import test from 'node:test';
import browser from '../../packages/sliccy-browser/src/browser.js';
import { fakeCdp } from './fake-cdp.mjs';

test('sliccy:browser ensureTab opens its tab in the background', { timeout: 30_000 }, async () => {
  const fake = await fakeCdp();
  const previous = process.env.SLICC_CDP_URL;
  process.env.SLICC_CDP_URL = fake.url;
  try {
    const blank = await browser.ensureTab('about:blank');
    const page = await browser.ensureTab('http://example.test/page');
    assert.notEqual(blank.targetId, page.targetId);
    const created = fake.calls.filter((call) => call.method === 'Target.createTarget');
    assert.equal(created.length, 2);
    for (const call of created) assert.equal(call.params?.background, true, JSON.stringify(call));
    assert.ok(fake.calls.some((call) => call.method === 'Page.navigate'));
    for (const method of [
      'Target.activateTarget',
      'Page.bringToFront',
      'Emulation.setFocusEmulationEnabled',
    ]) {
      assert.deepEqual(
        fake.calls.filter((call) => call.method === method),
        [],
        method
      );
    }
  } finally {
    if (previous === undefined) delete process.env.SLICC_CDP_URL;
    else process.env.SLICC_CDP_URL = previous;
    await fake.close();
  }
});
