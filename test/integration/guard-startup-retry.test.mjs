import assert from 'node:assert/strict';
import test from 'node:test';
import { waitExit } from './browser.mjs';
import { browserStartup } from './guard-fixture.mjs';

const timeout = () => new Error('debugging port 9300 did not answer /json/version (fetch failed)');

test('confirmed process exit cancels the cleanup timeout', async () => {
  await waitExit(Promise.resolve(), 5000);
});

test('unconfirmed process exit rejects cleanup and prevents startup retry', async () => {
  let calls = 0;
  const startup = browserStartup(async () => {
    calls += 1;
    await waitExit(new Promise(() => {}), 1);
  });
  await assert.rejects(startup.start([]), /chromium cleanup timed out/);
  assert.equal(calls, 1);
  assert.deepEqual(startup.retries, []);
});

test('successful browser startup needs no retry', async () => {
  let calls = 0;
  const browser = {};
  const startup = browserStartup(async (args) => {
    calls += 1;
    assert.deepEqual(args, ['--headless=new']);
    return browser;
  });
  assert.equal(await startup.start(['--headless=new']), browser);
  assert.equal(calls, 1);
  assert.deepEqual(startup.retries, []);
});

test('one initial JSON discovery timeout is retried and reported after cleanup', async () => {
  let calls = 0;
  let cleaned = false;
  const browser = {};
  const reported = [];
  const failure = timeout();
  const startup = browserStartup(
    async () => {
      calls += 1;
      if (calls === 1) {
        cleaned = true;
        throw failure;
      }
      assert.equal(cleaned, true);
      assert.equal(reported.length, 1);
      return browser;
    },
    (retry) => reported.push(retry)
  );
  assert.equal(await startup.start([], { target: 'ordinary', slot: 0 }), browser);
  assert.equal(calls, 2);
  assert.deepEqual(reported, [
    { target: 'ordinary', slot: 0, attempt: 1, reason: failure.message },
  ]);
  assert.deepEqual(startup.retries, reported);
  startup.retries[0].reason = 'changed';
  assert.equal(startup.retries[0].reason, failure.message);
});

test('a second startup timeout fails rather than being retried again', async () => {
  let calls = 0;
  const first = timeout();
  const second = timeout();
  const startup = browserStartup(async () => {
    calls += 1;
    throw calls === 1 ? first : second;
  });
  await assert.rejects(startup.start([]), (error) => error === second);
  assert.equal(calls, 2);
  assert.equal(startup.retries.length, 1);
  await assert.rejects(startup.start([]), (error) => error === second);
  assert.equal(calls, 3);
  assert.equal(startup.retries.length, 1);
});

test('startup is never retried after any row has begun', async () => {
  let calls = 0;
  const failure = timeout();
  const startup = browserStartup(async () => {
    calls += 1;
    throw failure;
  });
  startup.markRowStarted();
  await assert.rejects(startup.start([]), (error) => error === failure);
  assert.equal(calls, 1);
  assert.deepEqual(startup.retries, []);
});

test('a row starting during discovery closes the retry window', async () => {
  let fail;
  let calls = 0;
  const failure = timeout();
  const startup = browserStartup(() => {
    calls += 1;
    return new Promise((_, reject) => {
      fail = reject;
    });
  });
  const pending = startup.start([]);
  startup.markRowStarted();
  fail(failure);
  await assert.rejects(pending, (error) => error === failure);
  assert.equal(calls, 1);
  assert.deepEqual(startup.retries, []);
});

test('concurrent browser startups share one retry budget', async () => {
  const calls = new Map();
  const startup = browserStartup(async ([name]) => {
    const count = (calls.get(name) ?? 0) + 1;
    calls.set(name, count);
    if (count === 1) throw timeout();
    return name;
  });
  const results = await Promise.allSettled([startup.start(['a']), startup.start(['b'])]);
  assert.equal(results.filter((result) => result.status === 'fulfilled').length, 1);
  assert.equal(results.filter((result) => result.status === 'rejected').length, 1);
  assert.equal(
    [...calls.values()].reduce((sum, count) => sum + count, 0),
    3
  );
  assert.equal(startup.retries.length, 1);
});

test('guard failures and non-discovery launch errors are never retried', async () => {
  for (const failure of [
    new Error('Target.createTarget timed out'),
    new Error("slicc: refused: target is on seven's origin"),
    new Error('chromium exited code 1'),
    new Error('unexpected debugger url ws://example.test/'),
    new Error('Extensions.loadUnpacked failed'),
  ]) {
    let calls = 0;
    const startup = browserStartup(async () => {
      calls += 1;
      throw failure;
    });
    await assert.rejects(startup.start([]), (error) => error === failure);
    assert.equal(calls, 1);
    assert.deepEqual(startup.retries, []);
  }
});
