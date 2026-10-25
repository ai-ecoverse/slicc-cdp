import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';
import './watchdog.mjs';

const crashed = 'slicc: refused: renderer crashed\n';
const event = (kind, sessionId) =>
  kind === 'Target.detachedFromTarget'
    ? { method: kind, params: { sessionId, reason: 'target_crashed' } }
    : {
        method: kind,
        sessionId,
        params: kind === 'Inspector.detached' ? { reason: 'renderer crashed' } : {},
      };
const options = (transport) => ({ variant: 'prompt', detachDelay: 0, transport });
const page = (targetId) => ({ targetId, url: `https://${targetId.toLowerCase()}.example.test/` });

test('a fresh client observes the crash replayed by Inspector after the previous guard retired', async () => {
  const guards = new Set();
  let previous;
  let isCrashed = false;
  const fake = await fakeCdp({
    intercept(message, send) {
      if (message.method === 'Fetch.enable') guards.add(message.sessionId);
      if (message.method === 'Fetch.disable' && guards.has(message.sessionId) && !isCrashed) {
        previous = message.sessionId;
        isCrashed = true;
        send({ id: message.id, sessionId: message.sessionId, result: {} });
        send(event('Inspector.targetCrashed', message.sessionId));
        return true;
      }
      if (message.method === 'Inspector.enable' && isCrashed) {
        send(event('Inspector.targetCrashed', message.sessionId));
        send({ id: message.id, sessionId: message.sessionId, result: {} });
        return true;
      }
      return message.method === 'Network.enable' && isCrashed;
    },
  });
  fake.targets.push(page('USER'));
  let first;
  let second;
  let timer;
  const controller = new AbortController();
  try {
    first = await guarded(fake.url, 'https://private.test', options('native'));
    await first.send('Target.attachToTarget', { targetId: 'USER', flatten: true });
    await first.close();
    first = undefined;
    assert.ok(previous);
    const from = fake.calls.length;
    second = await guarded(fake.url, 'https://private.test', {
      ...options('native'),
      signal: controller.signal,
    });
    timer = setTimeout(() => controller.abort(), 8000);
    await assert.rejects(
      second.send('Target.attachToTarget', {
        targetId: 'USER',
        flatten: true,
      }),
      (error) => error.message === crashed
    );
    clearTimeout(timer);
    const fresh = fake.calls.slice(from);
    const inspecting = fresh.find((call) => call.method === 'Inspector.enable');
    assert.ok(inspecting);
    assert.notEqual(inspecting.sessionId, previous);
    assert.equal(inspecting.target, 'USER');
    assert.equal(
      fresh.some((call) => call.method === 'Network.enable'),
      false
    );
  } finally {
    clearTimeout(timer);
    controller.abort();
    await first?.close();
    await second?.close();
    await fake.close();
  }
});

for (const transport of ['native', 'wasi']) {
  for (const stage of ['Network.enable', 'Page.enable', 'Runtime.enable']) {
    test(`${transport}: renderer crash aborts guard setup at ${stage} even if a reply follows`, async () => {
      let injected = false;
      const fake = await fakeCdp({
        intercept(message, send) {
          if (!injected && message.method === stage) {
            injected = true;
            send(event('Inspector.targetCrashed', message.sessionId));
            send({ id: message.id, sessionId: message.sessionId, result: {} });
            return true;
          }
          return false;
        },
      });
      fake.targets.push(page('USER'));
      const client = await guarded(fake.url, 'https://private.test', options(transport));
      try {
        const started = Date.now();
        await assert.rejects(
          client.send('Target.attachToTarget', {
            targetId: 'USER',
            flatten: true,
          }),
          (error) => error.message === crashed
        );
        assert.ok(Date.now() - started < 5000);
        assert.equal(fake.calls.filter((call) => call.method === stage).length, 1);
      } finally {
        await client.close();
        await fake.close();
      }
    });
  }

  for (const kind of [
    'Inspector.targetCrashed',
    'Inspector.detached',
    'Target.detachedFromTarget',
  ]) {
    test(`${transport}: ${kind} on the guard aborts a sibling-session call without retry`, async () => {
      let active = false;
      let guard;
      const fake = await fakeCdp({
        intercept(message, send) {
          if (message.method === 'Fetch.enable') guard = message.sessionId;
          if (active && message.method === 'Network.enable') {
            active = false;
            send(event(kind, guard));
            send({ id: message.id, sessionId: message.sessionId, result: {} });
            return true;
          }
          return false;
        },
      });
      fake.targets.push(page('USER'));
      const client = await guarded(fake.url, 'https://private.test', options(transport));
      try {
        const { sessionId } = await client.send('Target.attachToTarget', {
          targetId: 'USER',
          flatten: true,
        });
        assert.notEqual(sessionId, guard);
        active = true;
        const started = Date.now();
        await assert.rejects(
          client.send('Network.enable', {}, sessionId),
          (error) => error.message === crashed
        );
        assert.ok(Date.now() - started < 5000);
        const from = fake.calls.length;
        await assert.rejects(
          client.send('Network.enable', {}, sessionId),
          (error) => error.message === crashed
        );
        await assert.rejects(
          client.send('Target.attachToTarget', {
            targetId: 'USER',
            flatten: true,
          }),
          (error) => error.message === crashed
        );
        assert.equal(
          fake.calls
            .slice(from)
            .some(
              (call) => call.method === 'Network.enable' || call.method === 'Target.attachToTarget'
            ),
          false
        );
        await client.send('Browser.getVersion');
      } finally {
        await client.close();
        await fake.close();
      }
    });
  }

  test(`${transport}: a different target's crash does not fail an ordinary call`, async () => {
    const guards = new Map();
    let active = false;
    const fake = await fakeCdp({
      intercept(message, send) {
        if (message.method === 'Fetch.enable')
          guards.set(fake.calls.at(-1).target, message.sessionId);
        if (active && message.method === 'Network.enable') {
          active = false;
          send(event('Inspector.targetCrashed', guards.get('OTHER')));
        }
        return false;
      },
    });
    fake.targets.push(page('USER'), page('OTHER'));
    const client = await guarded(fake.url, 'https://private.test', options(transport));
    try {
      const { sessionId } = await client.send('Target.attachToTarget', {
        targetId: 'USER',
        flatten: true,
      });
      const other = await client.send('Target.attachToTarget', {
        targetId: 'OTHER',
        flatten: true,
      });
      active = true;
      await client.send('Network.enable', {}, sessionId);
      await client.send('Browser.getVersion');
      await assert.rejects(
        client.send('Network.enable', {}, other.sessionId),
        (error) => error.message === crashed
      );
    } finally {
      await client.close();
      await fake.close();
    }
  });
}
