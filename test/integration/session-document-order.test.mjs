import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

for (const source of ['client', 'guard']) {
  for (const kind of ['BackForwardCacheRestore', 'Navigation']) {
    for (const copy of ['later', 'absent']) {
      for (const pending of [false, true]) {
        test(`guard refuses unchecked session documents (${source}, ${kind}, ${copy}, pending=${pending})`, async () => {
          const origin = 'https://private.test';
          const marker = randomUUID();
          const protectedUrl = `${origin}/controller/${marker}`;
          const frame = {
            id: 'PAGE',
            loaderId: 'L1',
            url: 'https://example.test/',
            securityOrigin: 'https://example.test',
          };
          const restored = {
            id: 'PAGE',
            loaderId: 'L0',
            url: 'chrome-error://chromewebdata/',
            securityOrigin: '://',
            unreachableUrl: protectedUrl,
          };
          const senders = new Map();
          const output = [];
          let sessionId;
          let guardSession;
          let armed = false;
          let release;
          const reached = Promise.withResolvers();
          let received = Promise.withResolvers();
          const event = (session) => ({
            method: 'Page.frameNavigated',
            sessionId: session,
            params: { type: kind, frame: { ...restored } },
          });
          const fake = await fakeCdp({
            intercept(message, send) {
              if (message.sessionId) senders.set(message.sessionId, send);
              if (message.method === 'Page.getFrameTree' && message.sessionId !== sessionId)
                guardSession = message.sessionId;
              if (message.method === 'Runtime.evaluate') {
                send({
                  id: message.id,
                  sessionId: message.sessionId,
                  result: { result: { type: 'string', value: 'ordinary' } },
                });
                return true;
              }
              if (
                armed &&
                message.sessionId === sessionId &&
                message.method ===
                  (pending ? 'Page.captureScreenshot' : 'Page.getNavigationHistory')
              ) {
                armed = false;
                send(event(source === 'client' ? sessionId : guardSession));
                if (pending) {
                  release = () =>
                    send({
                      id: message.id,
                      sessionId,
                      result: { data: Buffer.from(marker).toString('base64') },
                    });
                  reached.resolve();
                  return true;
                }
              }
              return false;
            },
          });
          fake.targets.push({ targetId: 'PAGE', type: 'page', url: frame.url, frame });
          const client = await guarded(fake.url, origin, {
            variant: 'prompt',
            detachDelay: 0,
            onMessage(message) {
              output.push(message);
              if (
                message.audit?.stage === 'received' &&
                message.audit.method === 'Runtime.evaluate'
              )
                received.resolve();
            },
          });
          const outcome = (promise) =>
            promise.then(
              () => ({ refused: false }),
              (error) => ({ refused: /slicc: refused:/.test(error.message) })
            );
          try {
            ({ sessionId } = await client.send('Target.attachToTarget', {
              targetId: 'PAGE',
              flatten: true,
            }));
            await client.send('Page.enable', {}, sessionId);
            await client.send('Runtime.evaluate', { expression: 'document.title' }, sessionId);
            const from = fake.calls.length;
            received = Promise.withResolvers();
            armed = true;
            let screenshot;
            if (pending) {
              screenshot = outcome(client.send('Page.captureScreenshot', {}, sessionId));
              await reached.promise;
            } else {
              await client.send('Page.getNavigationHistory', {}, sessionId);
            }
            const evaluation = outcome(
              client.send('Runtime.evaluate', { expression: 'document.title' }, sessionId)
            );
            if (pending) {
              await received.promise;
              release();
            }
            const evaluated = await evaluation;
            const captured = screenshot ? await screenshot : undefined;
            const forwarded = fake.calls
              .slice(from)
              .filter((call) => call.method === 'Runtime.evaluate');
            if (copy === 'later') {
              senders.get(guardSession)(event(guardSession));
              await outcome(client.send('Page.getNavigationHistory', {}, sessionId));
            }
            const text = JSON.stringify(output);
            assert.equal(text.includes(protectedUrl), false, 'protected URL is absent');
            assert.equal(text.includes(marker), false, 'document marker is absent');
            assert.equal(
              text.includes(Buffer.from(marker).toString('base64')),
              false,
              'image is absent'
            );
            assert.deepEqual(
              {
                refused: evaluated.refused,
                forwarded: forwarded.length,
                imageRefused: captured?.refused,
              },
              { refused: true, forwarded: 0, imageRefused: pending ? true : undefined }
            );
          } finally {
            release?.();
            await client.close();
            await fake.close();
          }
        });
      }
    }
  }
}
