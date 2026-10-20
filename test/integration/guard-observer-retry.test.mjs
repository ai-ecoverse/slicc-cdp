import assert from 'node:assert/strict';
import test from 'node:test';
import { observerSend } from './guard-client.mjs';

function transient() {
  const message = 'Inspected target navigated or closed';
  return Object.assign(new Error(message), { cdp: { code: -32000, message } });
}

test('observer retries a transient frame-tree navigation error without changing evidence', async () => {
  const frame = {
    url: 'chrome-error://chromewebdata/',
    securityOrigin: '://',
    unreachableUrl: 'https://seven.sliccy.ai/path?secret#fragment',
  };
  const result = { frameTree: { frame } };
  const params = {};
  let calls = 0;
  const send = observerSend(async (method, supplied, session) => {
    assert.equal(method, 'Page.getFrameTree');
    assert.equal(supplied, params);
    assert.equal(session, 'S');
    if (++calls < 3) throw transient();
    return result;
  });
  assert.equal(await send('Page.getFrameTree', params, 'S'), result);
  assert.equal(calls, 3);
});

test('observer caps retries rather than hiding a closed or unsettled target', async () => {
  const error = transient();
  let calls = 0;
  const send = observerSend(async () => {
    calls++;
    throw error;
  });
  await assert.rejects(send('Page.getFrameTree'), (seen) => seen === error);
  assert.equal(calls, 3);
});

test('observer never retries commands that drive a target', async () => {
  for (const method of [
    'Runtime.evaluate',
    'Page.navigate',
    'Page.reload',
    'Target.attachToTarget',
  ]) {
    let calls = 0;
    const send = observerSend(async () => {
      calls++;
      throw transient();
    });
    await assert.rejects(send(method));
    assert.equal(calls, 1);
  }
});

test('observer never retries other errors or loses their refusal', async () => {
  for (const error of [
    new Error('CDP connection closed'),
    Object.assign(new Error('No frame'), { cdp: { code: -32000, message: 'No frame' } }),
  ]) {
    let calls = 0;
    const send = observerSend(async () => {
      calls++;
      throw error;
    });
    await assert.rejects(send('Page.getFrameTree'), (seen) => seen === error);
    assert.equal(calls, 1);
  }
});
