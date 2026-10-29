import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

for (const transport of ['native', 'wasi']) {
  for (const extra of [
    'http://192.0.2.17:9443',
    'http://[2001:db8::17]:9443',
    'https://extra-fixture.invalid:9443',
  ]) {
    test(`${transport}: outer and nested fragments scrub ${extra}`, async () => {
      const hosts = [
        'vault.sliccy.ai',
        'seven.sliccy.ai',
        'tray-fixture.sliccy.ai',
        'controller-fixture.sliccy.ai',
        'VaUlT.SlIcCy.Ai',
        extra.split('://')[1],
      ];
      let title = 'ordinary';
      const info = { targetId: 'ORDINARY', type: 'page', url: 'https://ordinary.example/blank' };
      const fake = await fakeCdp({
        intercept(message, send) {
          if (!['Target.getTargets', 'Target.getTargetInfo'].includes(message.method)) return false;
          const targetInfo = { ...info, title };
          send({
            id: message.id,
            result:
              message.method === 'Target.getTargets'
                ? { targetInfos: [targetInfo] }
                : { targetInfo },
          });
          return true;
        },
      });
      fake.targets.push(info);
      const client = await guarded(fake.url, extra, {
        variant: 'prompt',
        detachDelay: 0,
        transport,
      });
      try {
        for (const prefix of [
          'https://ordinary.example/#',
          'https://ordinary.example/?a=https://x.example/#',
        ]) {
          for (const host of hosts) {
            for (const path of [
              '/controller/id-secret-fixture?key=fixture&sig=SECRETX',
              '/join/id-secret-fixture',
            ]) {
              title = prefix + host + path;
              const listing = await client.send('Target.getTargets');
              assert.equal(listing.targetInfos[0].title, prefix + host);
              assert.equal(listing.targetInfos[0].url, info.url);
              const result = await client.send('Target.getTargetInfo', { targetId: info.targetId });
              assert.equal(result.targetInfo.title, prefix + host);
              assert.equal(result.targetInfo.url, info.url);
            }
          }
        }
      } finally {
        await client.close();
        await fake.close();
      }
    });
  }
}
