import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

for (const mode of [
  'replaceState',
  'pushState',
  'fragment',
  'origin',
  'loader',
  'frame',
  'url-origin',
]) {
  for (const delivery of ['event', 'tree-only']) {
    test(`guard verifies same-document session updates (${mode}, ${delivery})`, async () => {
      const initial = {
        id: 'PAGE',
        loaderId: 'L',
        url: 'https://ordinary.test/start',
        securityOrigin: 'https://ordinary.test',
      };
      const current = {
        ...initial,
        url: mode === 'fragment' ? initial.url : 'https://ordinary.test/updated?state=next',
        urlFragment: '#loaded',
        ...(mode === 'origin'
          ? { url: 'https://other.test/current', securityOrigin: 'https://other.test' }
          : {}),
        ...(mode === 'url-origin' ? { url: 'https://other.test/current' } : {}),
        ...(mode === 'loader' ? { loaderId: 'NEW' } : {}),
        ...(mode === 'frame' ? { id: 'OTHER' } : {}),
      };
      const allowed = ['replaceState', 'pushState', 'fragment'].includes(mode);
      const page = { targetId: 'PAGE', type: 'page', url: initial.url, frame: initial };
      let guardSession;
      let armed = false;
      const fake = await fakeCdp({
        intercept(message, send) {
          if (message.method === 'Page.getFrameTree') guardSession = message.sessionId;
          if (armed && message.method === 'Page.getNavigationHistory') {
            armed = false;
            for (const sessionId of [guardSession, message.sessionId])
              send({ method: 'Page.frameNavigated', sessionId, params: { frame: initial } });
            page.frame = current;
            page.url = `${current.url}${current.urlFragment}`;
            if (delivery === 'event')
              send({
                method: 'Page.navigatedWithinDocument',
                sessionId: message.sessionId,
                params: {
                  frameId: 'PAGE',
                  url: page.url,
                  navigationType: mode === 'fragment' ? 'fragment' : 'historyApi',
                },
              });
          }
          if (message.method === 'Runtime.evaluate') {
            send({
              id: message.id,
              sessionId: message.sessionId,
              result: { result: { type: 'string', value: 'ordinary' } },
            });
            return true;
          }
          return false;
        },
      });
      fake.targets.push(page);
      const client = await guarded(fake.url, 'https://private.test', {
        variant: 'prompt',
        detachDelay: 0,
      });
      try {
        const { sessionId } = await client.send('Target.attachToTarget', {
          targetId: 'PAGE',
          flatten: true,
        });
        await client.send('Runtime.evaluate', { expression: 'document.title' }, sessionId);
        armed = true;
        await client.send('Page.getNavigationHistory', {}, sessionId);
        const from = fake.calls.length;
        const read = () =>
          client.send('Runtime.evaluate', { expression: 'document.title' }, sessionId);
        if (allowed) {
          assert.equal((await read()).result.value, 'ordinary');
          assert.equal((await read()).result.value, 'ordinary');
        } else await assert.rejects(read(), /slicc: refused:/);
        assert.equal(
          fake.calls.slice(from).filter((call) => call.method === 'Runtime.evaluate').length,
          allowed ? 2 : 0
        );
      } finally {
        await client.close();
        await fake.close();
      }
    });
  }
}
