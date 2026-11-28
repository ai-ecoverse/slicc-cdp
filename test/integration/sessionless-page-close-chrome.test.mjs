import assert from 'node:assert/strict';
import { launchBrowser } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import './watchdog.mjs';

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function browserSocket(wsUrl) {
  const ws = new WebSocket(wsUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener('open', () => resolve(), { once: true });
    ws.addEventListener('error', () => reject(new Error('browser websocket failed')), {
      once: true,
    });
  });
  const waiters = new Map();
  let next = 0;
  ws.addEventListener('message', (event) => {
    const text = typeof event.data === 'string' ? event.data : new TextDecoder().decode(event.data);
    const message = JSON.parse(text);
    if (message.id === undefined) return;
    const waiter = waiters.get(message.id);
    if (!waiter) return;
    waiters.delete(message.id);
    waiter(message);
  });
  const raw = (body, ms) => {
    const id = ++next;
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        waiters.delete(id);
        resolve({ id, reply: null });
      }, ms);
      waiters.set(id, (message) => {
        clearTimeout(timer);
        resolve({ id, reply: message });
      });
      ws.send(JSON.stringify({ id, ...body }));
    });
  };
  const send = async (method, params = {}, sessionId) => {
    const { reply } = await raw({ method, params, ...(sessionId ? { sessionId } : {}) }, 30000);
    if (!reply) throw new Error(`${method} timed out`);
    if (reply.error) throw new Error(`${method} ${JSON.stringify(reply.error)}`);
    return reply.result ?? {};
  };
  return { raw, send, close: () => ws.close() };
}

async function survives(socket, targetId) {
  const until = Date.now() + 3000;
  for (;;) {
    const { targetInfos } = await socket.send('Target.getTargets');
    if (!targetInfos.some((info) => info.targetId === targetId)) return false;
    if (Date.now() >= until) return true;
    await delay(100);
  }
}

test('the browser endpoint answers a page command without a session with method-not-found and the page survives', async () => {
  let browser;
  let socket;
  try {
    browser = await launchBrowser();
    socket = await browserSocket(browser.wsUrl);
    const { targetId: a } = await socket.send('Target.createTarget', { url: 'about:blank' });
    const { targetId: b } = await socket.send('Target.createTarget', { url: 'about:blank' });
    for (const [variant, body] of [
      ['noparams', { method: 'Page.close' }],
      ['emptyparams', { method: 'Page.close', params: {} }],
    ]) {
      const { id, reply } = await socket.raw(body, 5000);
      assert.ok(reply && typeof reply === 'object', `${variant}: a reply arrived`);
      assert.equal(reply.id, id, variant);
      assert.equal(reply.error?.code, -32601, `${variant}: ${JSON.stringify(reply)}`);
      assert.match(reply.error.message, /Page\.close/, variant);
      assert.equal(Object.hasOwn(reply, 'result'), false, variant);
      assert.equal(await survives(socket, a), true, `${variant}: the page survives`);
    }
    const { sessionId } = await socket.send('Target.attachToTarget', {
      targetId: b,
      flatten: true,
    });
    const control = await socket.raw({ method: 'Page.close', params: {}, sessionId }, 5000);
    assert.ok(control.reply, 'the control Page.close got a reply');
    assert.equal(control.reply.id, control.id);
    assert.equal(control.reply.error, undefined, JSON.stringify(control.reply));
    assert.deepEqual(control.reply.result, {});
    assert.equal(await survives(socket, b), false, 'the control page is gone');
  } finally {
    socket?.close();
    await browser?.close();
  }
});
