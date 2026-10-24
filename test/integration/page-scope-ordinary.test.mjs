import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

const origin = 'https://ordinary.test';
const frame = (id, url = `${origin}/`, securityOrigin = origin) => ({
  id,
  loaderId: `L${id}`,
  url,
  securityOrigin,
});

for (const layout of ['local', 'remote']) {
  for (const context of ['ordinary', 'protected', 'missing']) {
    test(`page scope ${layout} inherited context ${context}`, async () => {
      const trees = { P: { frame: frame('P') } };
      const owner = layout === 'local' ? 'P' : 'M';
      if (layout === 'remote') trees.M = { frame: frame('M') };
      trees[owner].childFrames = [{ frame: frame('INHERITED', 'about:srcdoc', '://') }];
      let fake;
      fake = await fakeCdp({
        discoverEvents: true,
        intercept(message, send) {
          if (message.method !== 'Page.getFrameTree') return false;
          send({ id: message.id, result: { frameTree: trees[fake.calls.at(-1).target] } });
          return true;
        },
        onCommand(message, event) {
          const target = fake.calls.at(-1).target;
          if (message.method === 'Page.enable') {
            event('Target.targetInfoChanged', {
              targetInfo: {
                targetId: target,
                type: target === 'P' ? 'page' : 'iframe',
                url: `${origin}/`,
                ...(target === 'M' ? { parentFrameId: 'P' } : {}),
                attached: true,
                title: 'Ordinary',
                browserContextId: 'CTX',
              },
            });
          }
          if (message.method === 'Runtime.enable' && target === owner && context !== 'missing') {
            event('Runtime.executionContextCreated', {
              context: {
                id: 99,
                uniqueId: 'inherited-context',
                name: '',
                origin: context === 'ordinary' ? origin : 'https://protected.test',
                auxData: { isDefault: true, frameId: 'INHERITED' },
              },
            });
          }
        },
      });
      fake.targets.push({ targetId: 'P', type: 'page', url: `${origin}/` });
      if (layout === 'remote')
        fake.targets.push({ targetId: 'M', type: 'iframe', url: `${origin}/`, parentFrameId: 'P' });
      const client = await guarded(fake.url, 'https://protected.test', {
        variant: 'prompt',
        detachDelay: 0,
      });
      try {
        const attached = await client
          .send('Target.attachToTarget', { targetId: 'P', flatten: true })
          .then(
            (result) => ({ result }),
            (error) => ({ error })
          );
        if (context === 'ordinary') {
          assert.equal(attached.error, undefined, attached.error?.message);
          const session = attached.result.sessionId;
          if (layout === 'local')
            await client.send('Page.createIsolatedWorld', { frameId: 'INHERITED' }, session);
          for (const method of [
            'Page.captureScreenshot',
            'DOM.getDocument',
            'Accessibility.getFullAXTree',
            'Input.dispatchKeyEvent',
          ])
            await client.send(method, {}, session);
          await client.send('DOM.getDocument', { depth: -1, pierce: true }, session);
          await client.send('DOMSnapshot.captureSnapshot', { computedStyles: [] }, session);
        } else if (attached.error) assert.match(attached.error.message, /slicc: refused:/);
        else
          await assert.rejects(
            client.send('Page.captureScreenshot', {}, attached.result.sessionId),
            /slicc: refused:/
          );
      } finally {
        await client.close();
        await fake.close();
      }
    });
  }
}

test('page scope retains ordinary input during title and unrelated frame changes', async () => {
  let fake;
  fake = await fakeCdp({
    discoverEvents: true,
    onCommand(message, event) {
      if (message.method !== 'Input.dispatchKeyEvent') return;
      event('Target.targetInfoChanged', {
        targetInfo: {
          targetId: 'P',
          type: 'page',
          url: `${origin}/`,
          title: 'Changed',
          attached: true,
          browserContextId: 'CTX',
        },
      });
      const page = fake.targets.find((target) => target.targetId === 'P');
      page.url = `${origin}/next`;
      page.loaderId = 'NEXT';
      event('Page.frameNavigated', {
        frame: { id: 'P', loaderId: 'NEXT', url: page.url, securityOrigin: origin },
      });
      fake.targets.find((target) => target.targetId === 'V').url = `${origin}/next`;
      event('Target.targetInfoChanged', {
        targetInfo: {
          targetId: 'V',
          type: 'iframe',
          parentFrameId: 'Q',
          url: `${origin}/next`,
          title: 'Changed',
          attached: true,
          browserContextId: 'CTX',
        },
      });
    },
  });
  fake.targets.push({ targetId: 'P', type: 'page', url: `${origin}/` });
  fake.targets.push({ targetId: 'Q', type: 'page', url: `${origin}/` });
  fake.targets.push({
    targetId: 'V',
    type: 'iframe',
    parentFrameId: 'Q',
    url: `${origin}/`,
    tree: { frame: { ...frame('V'), parentId: 'Q' } },
  });
  const client = await guarded(fake.url, 'https://protected.test', {
    variant: 'prompt',
    detachDelay: 0,
  });
  try {
    const { sessionId } = await client.send('Target.attachToTarget', {
      targetId: 'P',
      flatten: true,
    });
    await client.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter' }, sessionId);
    assert.equal(fake.calls.filter((call) => call.method === 'Input.dispatchKeyEvent').length, 1);
  } finally {
    await client.close();
    await fake.close();
  }
});
