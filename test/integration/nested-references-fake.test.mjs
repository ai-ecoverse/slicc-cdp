import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

const before = 'https://ordinary.example/page?keep=visible%20bytes%2B%41&to=';
const after = '&end=visible%252Fbytes#ordinary-fragment';
const host = 'vault.sliccy.ai';
const expected = before + host + after;
const encoded = [
  ...new TextEncoder().encode(host + '/doc/path-secret-fixture?query=fixture#fragment'),
]
  .map((byte) => `%${byte.toString(16).toUpperCase().padStart(2, '0')}`)
  .join('');
const references = [
  host + '/doc/path-secret-fixture?query=fixture#fragment',
  host + '%2Fdoc%2Fpath-secret-fixture%3Fquery%3Dfixture%23fragment',
  encoded.replaceAll('%', '%25'),
];

for (const transport of ['native', 'wasi']) {
  test(`${transport}: nested references in discovery URLs preserve ordinary metadata`, async () => {
    let notify = false;
    const fake = await fakeCdp({
      intercept(message, send) {
        if (message.method !== 'Browser.getVersion' || !notify) return false;
        notify = false;
        for (const method of ['Target.targetCreated', 'Target.targetInfoChanged'])
          send({
            method,
            params: {
              targetInfo: { ...fake.targets[0], type: 'page', title: fake.targets[0].url },
            },
          });
        return false;
      },
    });
    fake.targets.push({ targetId: 'ORDINARY', type: 'page', url: before + references[0] + after });
    const client = await guarded(fake.url, 'https://private.example.invalid', {
      variant: 'prompt',
      detachDelay: 0,
      transport,
    });
    try {
      for (const reference of references) {
        fake.targets[0].url = before + reference + after;
        const listing = await client.send('Target.getTargets');
        assert.equal(listing.targetInfos[0].url, expected);
        assert.equal(listing.targetInfos[0].title, expected);
        const info = await client.send('Target.getTargetInfo', { targetId: 'ORDINARY' });
        assert.equal(info.targetInfo.url, expected);
        assert.equal(info.targetInfo.title, expected);
        const from = client.events.length;
        notify = true;
        await client.send('Browser.getVersion');
        await client.send('Browser.getVersion');
        for (const method of ['Target.targetCreated', 'Target.targetInfoChanged']) {
          const event = client.events.slice(from).find((event) => event.method === method);
          assert.ok(event);
          assert.equal(event.params.targetInfo.url, expected);
          assert.equal(event.params.targetInfo.title, expected);
        }
      }
    } finally {
      await client.close();
      await fake.close();
    }
  });
}
