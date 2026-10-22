import assert from 'node:assert/strict';
import test from 'node:test';
import { fixtureServer } from '@ai-ecoverse/slicc-browsers/fixture';
import { launchBrowser } from './browser.mjs';
import { guarded, observer } from './guard-client.mjs';
import { browserArgs, origins } from './guard-fixture.mjs';

async function navigate(raw, session, url) {
  const loaded = raw.waitEvent(
    (event) => event.method === 'Page.loadEventFired' && event.sessionId === session,
    15000
  );
  await raw.send('Page.navigate', { url }, session);
  await loaded;
}

async function read(raw, session, contextId) {
  return (
    await raw.send(
      'Runtime.evaluate',
      {
        expression:
          '({injected:globalThis.__persistent_probe === true,binding:typeof globalThis.persistentProbeBinding})',
        returnByValue: true,
        ...(contextId ? { contextId } : {}),
      },
      session
    )
  ).result.value;
}

test('persistent page capabilities are denied before later documents', {
  timeout: 180000,
}, async (t) => {
  const fixture = await fixtureServer();
  const sites = origins(fixture);
  try {
    for (const variant of ['navigation', 'history', 'child']) {
      await t.test(variant, async () => {
        const browser = await launchBrowser(browserArgs());
        const raw = await observer(browser.wsUrl);
        const client = await guarded(browser.wsUrl, sites.client, {
          variant: 'prompt',
          detachDelay: 2000,
        });
        try {
          const { targetId } = await raw.send('Target.createTarget', {
            url: 'about:blank',
            background: true,
          });
          const { sessionId } = await raw.send('Target.attachToTarget', {
            targetId,
            flatten: true,
          });
          await raw.send('Page.enable', {}, sessionId);
          if (variant === 'history') await navigate(raw, sessionId, `${sites.seven}/doc`);
          await navigate(raw, sessionId, `${sites.ordinary}/doc`);
          const { sessionId: held } = await client.send('Target.attachToTarget', {
            targetId,
            flatten: true,
          });
          for (const [method, params] of [
            [
              'Page.addScriptToEvaluateOnNewDocument',
              { source: 'globalThis.__persistent_probe = true' },
            ],
            ['Page.setBypassCSP', { enabled: true }],
            ['Network.setExtraHTTPHeaders', { headers: { 'x-persistent-probe': 'changed' } }],
            ['Emulation.setScriptExecutionDisabled', { value: true }],
          ])
            await assert.rejects(client.send(method, params, held), /slicc: refused:/);
          await client.send('Runtime.enable', {}, held);
          const context = client.events.find(
            (event) =>
              event.method === 'Runtime.executionContextCreated' &&
              event.sessionId === held &&
              event.params.context.auxData?.isDefault
          )?.params.context;
          assert.ok(context, 'a verified ordinary main-world context was observed');
          await assert.rejects(
            client.send(
              'Runtime.addBinding',
              { name: 'persistentProbeBinding', executionContextId: context.id },
              held
            ),
            /slicc: refused:/
          );
          if (variant === 'navigation') {
            await navigate(raw, sessionId, `${sites.vault}/doc`);
          } else if (variant === 'history') {
            const history = await raw.send('Page.getNavigationHistory', {}, sessionId);
            const protectedEntry = history.entries.find(
              (entry) => entry.url === `${sites.seven}/doc`
            );
            assert.ok(protectedEntry);
            const navigated = raw.waitEvent(
              (event) => event.method === 'Page.frameNavigated' && event.sessionId === sessionId,
              15000
            );
            await raw.send(
              'Page.navigateToHistoryEntry',
              { entryId: protectedEntry.id },
              sessionId
            );
            await navigated;
          } else {
            await raw.send(
              'Runtime.evaluate',
              {
                expression: `new Promise(resolve=>{const frame=document.createElement('iframe');frame.onload=()=>resolve(true);frame.src=${JSON.stringify(`${sites.seven}/doc`)};document.body.append(frame);})`,
                returnByValue: true,
                awaitPromise: true,
              },
              sessionId
            );
            const tree = await raw.send('Page.getFrameTree', {}, sessionId);
            const frame = tree.frameTree.childFrames?.[0]?.frame;
            assert.ok(frame);
            const targets = await raw.send('Target.getTargets', {});
            const oopif = targets.targetInfos.find(
              (info) => info.type === 'iframe' && info.targetId === frame.id
            );
            if (oopif) {
              const { sessionId: child } = await raw.send('Target.attachToTarget', {
                targetId: oopif.targetId,
                flatten: true,
              });
              assert.deepEqual(await read(raw, child), { injected: false, binding: 'undefined' });
            } else {
              await raw.send('Runtime.enable', {}, sessionId);
              const childContext = raw.events.findLast(
                (event) =>
                  event.method === 'Runtime.executionContextCreated' &&
                  event.sessionId === sessionId &&
                  event.params.context.auxData?.isDefault &&
                  event.params.context.auxData.frameId === frame.id
              )?.params.context;
              assert.ok(childContext);
              assert.deepEqual(await read(raw, sessionId, childContext.id), {
                injected: false,
                binding: 'undefined',
              });
            }
          }
          assert.deepEqual(await read(raw, sessionId), { injected: false, binding: 'undefined' });
          assert.ok(fixture.log().every((entry) => !entry.headers['x-persistent-probe']));
        } finally {
          await client.close();
          await raw.close();
          await browser.close();
        }
      });
    }
  } finally {
    await fixture.close();
  }
});
