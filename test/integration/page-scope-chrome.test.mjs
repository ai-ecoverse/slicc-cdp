import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { inflateSync } from 'node:zlib';
import { launchBrowser } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { guarded, observer } from './guard-client.mjs';
import './watchdog.mjs';

function pixels(data) {
  const png = Buffer.from(data.replace(/^data:[^,]*,/, ''), 'base64');
  assert.equal(png.subarray(1, 4).toString(), 'PNG');
  const width = png.readUInt32BE(16);
  const height = png.readUInt32BE(20);
  assert.equal(png[24], 8);
  assert.ok([2, 6].includes(png[25]));
  const channels = png[25] === 6 ? 4 : 3;
  const chunks = [];
  for (let offset = 8; offset < png.length; ) {
    const size = png.readUInt32BE(offset);
    if (png.subarray(offset + 4, offset + 8).toString() === 'IDAT')
      chunks.push(png.subarray(offset + 8, offset + 8 + size));
    offset += size + 12;
  }
  const bytes = inflateSync(Buffer.concat(chunks));
  const stride = width * channels;
  let previous = Buffer.alloc(stride);
  let marked = 0;
  for (let y = 0; y < height; y++) {
    const filter = bytes[y * (stride + 1)];
    const row = Buffer.from(bytes.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1)));
    for (let x = 0; x < stride; x++) {
      const left = x >= channels ? row[x - channels] : 0;
      const above = previous[x];
      const corner = x >= channels ? previous[x - channels] : 0;
      const prediction = left + above - corner;
      const distances = [
        Math.abs(prediction - left),
        Math.abs(prediction - above),
        Math.abs(prediction - corner),
      ];
      const paeth =
        distances[0] <= distances[1] && distances[0] <= distances[2]
          ? left
          : distances[1] <= distances[2]
            ? above
            : corner;
      assert.ok(filter <= 4);
      row[x] = (row[x] + [0, left, above, Math.floor((left + above) / 2), paeth][filter]) & 255;
    }
    for (let x = 0; x < stride; x += channels)
      if (row[x] === 241 && row[x + 1] === 17 && row[x + 2] === 223) marked++;
    previous = row;
  }
  return marked;
}

async function site(page) {
  const server = createServer((req, res) => {
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end(`<!doctype html>${page(req.url)}`);
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return {
    port: server.address().port,
    close: () =>
      new Promise((resolve) => {
        server.closeAllConnections();
        server.close(resolve);
      }),
  };
}

async function until(run) {
  const end = Date.now() + 15000;
  do {
    const result = await run();
    if (result) return result;
    await new Promise((resolve) => setTimeout(resolve, 50));
  } while (Date.now() < end);
  throw new Error('page scope fixture deadline');
}

const surfaces = [
  ['Page.captureScreenshot', { format: 'png' }],
  ['Page.printToPDF', {}],
  ['DOM.getDocument', { depth: 1 }],
  ['DOM.getDocument', { depth: -1, pierce: true }],
  ['DOMSnapshot.captureSnapshot', { computedStyles: [] }],
  ['Accessibility.getFullAXTree', {}],
  ['Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter' }],
  [
    'Input.dispatchMouseEvent',
    { type: 'mousePressed', x: 10, y: 10, button: 'left', clickCount: 1 },
  ],
];

for (const layout of ['a', 'b', 'c']) {
  test(`page scope ${layout} capture and input controls`, { timeout: 180000 }, async (t) => {
    const marker =
      '<style>html,body{margin:0;background:rgb(241,17,223);height:280px}</style><button>Frame marker</button>';
    const inner = await site(
      () =>
        `<iframe name="inherited" style="width:300px;height:280px;border:0" srcdoc="${marker.replaceAll('"', '&quot;')}"></iframe>`
    );
    const ordinary = await site((url) =>
      url === '/middle'
        ? `<iframe name="child" src="http://localhost:${inner.port}/"></iframe>`
        : `<button>Top</button><iframe name="middle" style="width:400px;height:360px;border:0" src="http://localhost:${url === '/c' ? ordinary.port : inner.port}/${url === '/c' ? 'middle' : ''}"></iframe>`
    );
    let browser;
    let raw;
    let client;
    try {
      browser = await launchBrowser(['--site-per-process'], { headless: true });
      raw = await observer(browser.wsUrl);
      const url = `http://${layout === 'a' ? 'localhost' : '127.0.0.1'}:${ordinary.port}/${layout}`;
      const { targetId } = await raw.send('Target.createTarget', { url });
      const rawSession = (await raw.send('Target.attachToTarget', { targetId, flatten: true }))
        .sessionId;
      await raw.send('Page.enable', {}, rawSession);
      await raw.send('Target.activateTarget', { targetId });
      const marked = await until(
        async () =>
          pixels((await raw.send('Page.captureScreenshot', { format: 'png' }, rawSession)).data) >
          1000
      );
      assert.equal(marked, true);
      const targets = (await raw.send('Target.getTargets', { filter: [{}] })).targetInfos;
      const middle = targets.find(
        (info) =>
          info.type === 'iframe' &&
          info.url ===
            `http://localhost:${layout === 'c' ? ordinary.port : inner.port}/${layout === 'c' ? 'middle' : ''}`
      );
      if (layout === 'a') assert.equal(middle, undefined);
      else assert.ok(middle);
      if (layout === 'c')
        assert.equal(
          targets.some(
            (info) => info.type === 'iframe' && info.url === `http://localhost:${inner.port}/`
          ),
          false
        );
      const controlCast = raw.events.length;
      await raw.send('Page.startScreencast', { format: 'png', everyNthFrame: 1 }, rawSession);
      await until(() =>
        raw.events
          .slice(controlCast)
          .find(
            (event) =>
              event.method === 'Page.screencastFrame' &&
              event.sessionId === rawSession &&
              pixels(event.params.data) > 1000
          )
      );
      await raw.send('Page.stopScreencast', {}, rawSession);
      for (const ordinaryControl of [false, true]) {
        client = await guarded(
          browser.wsUrl,
          ordinaryControl ? 'http://localhost:1' : `http://localhost:${inner.port}`,
          { variant: 'prompt', detachDelay: 0 }
        );
        const attached = await client
          .send('Target.attachToTarget', { targetId, flatten: true })
          .then(
            (result) => ({ result }),
            (error) => ({ error })
          );
        const sessionId = attached.result?.sessionId;
        if (ordinaryControl) assert.ok(sessionId, attached.error?.message);
        const start = client.events.length;
        const check = async (method, params) => {
          await t.test(`${method} ${ordinaryControl ? 'ordinary' : 'protected'}`, async () => {
            const outcome = attached.error
              ? attached
              : await client.send(method, params, sessionId).then(
                  (result) => ({ result }),
                  (error) => ({ error })
                );
            const markerPixels =
              method === 'Page.captureScreenshot' && outcome.result?.data
                ? pixels(outcome.result.data)
                : 0;
            if (method === 'Page.captureScreenshot')
              t.diagnostic(
                JSON.stringify({
                  layout,
                  ordinary: ordinaryControl,
                  markerPixels,
                  refused: Boolean(outcome.error),
                })
              );
            if (ordinaryControl) {
              assert.equal(outcome.error, undefined, outcome.error?.message);
              if (method === 'Page.captureScreenshot') assert.ok(markerPixels > 1000);
              if (method === 'Page.printToPDF')
                assert.ok(Buffer.from(outcome.result.data, 'base64').length > 100);
            } else {
              assert.equal(markerPixels, 0);
              assert.match(outcome.error?.message ?? '', /slicc: refused:/);
              assert.equal(outcome.result?.data ?? '', '');
            }
          });
        };
        for (const [method, params] of surfaces) await check(method, params);
        if (attached.error) assert.match(attached.error.message, /slicc: refused:/);
        else
          await assert.rejects(
            client.send('Page.startScreencast', { format: 'png' }, sessionId),
            /slicc: refused:/
          );
        assert.equal(
          client.events.slice(start).filter((event) => event.method === 'Page.screencastFrame')
            .length,
          0
        );
        if (ordinaryControl) await client.send('CSS.enable', {}, sessionId);
        const dom = await (ordinaryControl ? client : raw).send(
          'DOM.getDocument',
          { depth: 1 },
          ordinaryControl ? sessionId : rawSession
        );
        const nodeId = dom.root.children.find((node) => node.nodeType === 1).nodeId;
        for (const [method, params] of [
          ['DOM.getOuterHTML', { nodeId }],
          ['CSS.getComputedStyleForNode', { nodeId }],
          ['Accessibility.getPartialAXTree', { nodeId }],
        ])
          await check(method, params);
        if (middle) {
          const frameAttach = await client
            .send('Target.attachToTarget', { targetId: middle.targetId, flatten: true })
            .then(
              (result) => ({ result }),
              (error) => ({ error })
            );
          if (ordinaryControl) {
            assert.equal(frameAttach.error, undefined, frameAttach.error?.message);
            await client.send(
              'Page.captureScreenshot',
              { format: 'png' },
              frameAttach.result.sessionId
            );
            await client.send('DOM.getDocument', { depth: 1 }, frameAttach.result.sessionId);
          } else if (frameAttach.error) assert.match(frameAttach.error.message, /slicc: refused:/);
          else
            await assert.rejects(
              client.send(
                'Page.captureScreenshot',
                { format: 'png' },
                frameAttach.result.sessionId
              ),
              /slicc: refused:/
            );
        }
        await client.close();
        client = undefined;
      }
    } finally {
      await client?.close();
      await raw?.close();
      await browser?.close();
      await ordinary.close();
      await inner.close();
    }
  });
}
