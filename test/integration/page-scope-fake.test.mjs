import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

const surfaces = [
  ['Page.captureScreenshot', {}],
  ['Page.printToPDF', {}],
  ['DOM.getDocument', { depth: 1 }],
  ['DOM.describeNode', { nodeId: 1 }],
  ['DOM.getOuterHTML', { nodeId: 1 }],
  ['DOMSnapshot.captureSnapshot', { computedStyles: [] }],
  ['Accessibility.getFullAXTree', {}],
  ['Accessibility.getPartialAXTree', { nodeId: 1 }],
  ['CSS.getComputedStyleForNode', { nodeId: 1 }],
  ['Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter' }],
  ['Input.dispatchMouseEvent', { type: 'mousePressed', x: 1, y: 1, button: 'left', clickCount: 1 }],
];

function node(id, parentId, protectedFrame = false) {
  const origin = protectedFrame ? 'http://localhost:9401' : 'http://localhost:9400';
  return {
    frame: {
      id,
      ...(parentId ? { parentId } : {}),
      loaderId: `L${id}`,
      url: `${origin}/`,
      securityOrigin: origin,
    },
  };
}

async function fixture(layout, protectedFrame, run) {
  const trees = { TOP: node('TOP') };
  const targets = [{ targetId: 'TOP', type: 'page', url: trees.TOP.frame.url }];
  if (layout === 'a') {
    trees.TOP.childFrames = [node('CHILD', 'TOP', protectedFrame)];
    trees.TOP.childFrames[0].childFrames = [
      { frame: { ...node('INHERITED', 'CHILD', protectedFrame).frame, url: 'about:srcdoc' } },
    ];
  } else {
    const middle = node('MIDDLE', 'TOP', layout === 'b' && protectedFrame);
    targets.push({
      targetId: 'MIDDLE',
      type: 'iframe',
      parentFrameId: 'TOP',
      url: middle.frame.url,
    });
    trees.MIDDLE = middle;
    if (layout === 'c') trees.MIDDLE.childFrames = [node('CHILD', 'MIDDLE', protectedFrame)];
  }
  const fake = await fakeCdp({
    discoverEvents: true,
    intercept(message, send) {
      if (message.method !== 'Page.getFrameTree') return false;
      const target = fake.calls.at(-1)?.target;
      send({ id: message.id, result: { frameTree: trees[target] ?? trees.TOP } });
      return true;
    },
  });
  fake.targets.push(...targets);
  const client = await guarded(fake.url, 'http://localhost:9401', {
    variant: 'prompt',
    detachDelay: 0,
  });
  try {
    const { sessionId } = await client.send('Target.attachToTarget', {
      targetId: 'TOP',
      flatten: true,
    });
    await run(client, sessionId, fake, trees);
  } finally {
    await client.close();
    await fake.close();
  }
}

for (const layout of ['a', 'b', 'c']) {
  test(`page scope ${layout} refuses protected content`, async () => {
    await fixture(layout, true, async (client, session, fake) => {
      for (const [method, params] of surfaces) {
        const from = fake.calls.length;
        await assert.rejects(client.send(method, params, session), /slicc: refused:/);
        assert.equal(
          fake.calls.slice(from).some((call) => call.method === method),
          false,
          method
        );
      }
      for (const method of [
        'Page.startScreencast',
        'LayerTree.makeSnapshot',
        'LayerTree.replaySnapshot',
        'LayerTree.profileSnapshot',
        'LayerTree.snapshotCommandLog',
        'HeadlessExperimental.beginFrame',
        'Cast.startTabMirroring',
      ]) {
        await assert.rejects(client.send(method, {}, session), /slicc: refused:/);
      }
    });
  });
  test(`page scope ${layout} retains ordinary content`, async () => {
    await fixture(layout, false, async (client, session, fake) => {
      for (const [method, params] of surfaces) {
        const from = fake.calls.length;
        await client.send(method, params, session);
        assert.equal(
          fake.calls.slice(from).filter((call) => call.method === method).length,
          1,
          method
        );
      }
    });
  });
  test(`page scope ${layout} checks late replies`, async () => {
    for (const method of ['Page.captureScreenshot', 'Page.printToPDF']) {
      await fixture(layout, false, async (client, session, fake, trees) => {
        const push = fake.calls.push.bind(fake.calls);
        fake.calls.push = (call) => {
          if (call.method === method) {
            const tree = layout === 'a' ? trees.TOP : trees.MIDDLE;
            tree.childFrames = [node('LATE', tree.frame.id, true)];
          }
          return push(call);
        };
        await assert.rejects(client.send(method, {}, session), /slicc: refused:/);
      });
    }
  });
}
