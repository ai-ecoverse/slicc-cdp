import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';
import './watchdog.mjs';

const ERROR = 'chrome-error://chromewebdata/';
const VAULT = 'https://vault.sliccy.ai/private';
const AFTER = 'https://example.test/after';
const touching = new Set([
  'Runtime.evaluate',
  'Runtime.callFunctionOn',
  'Page.createIsolatedWorld',
]);

function errorTab(reported, previous = 'about:blank') {
  const unreachableUrl = reported === ERROR ? VAULT : reported;
  return {
    targetId: '1790572252',
    url: reported,
    frame: { id: 'FRAME', loaderId: 'ERROR', url: ERROR, securityOrigin: '://', unreachableUrl },
    history: {
      currentIndex: 1,
      entries: [
        { id: 17, url: previous, userTypedURL: previous, transitionType: 'link' },
        { id: 22, url: unreachableUrl, userTypedURL: '', transitionType: 'link' },
      ],
    },
  };
}

async function fixture(t, tabs, options = {}) {
  let emit;
  const fake = await fakeCdp({
    tabSessions: true,
    ...options,
    intercept: (_message, send) => {
      emit = send;
      return false;
    },
  });
  fake.targets.push(...tabs);
  t.after(() => fake.close());
  const client = await guarded(fake.url, 'https://seven.sliccy.ai', {
    variant: 'prompt',
    detachDelay: 0,
    transport: 'native',
  });
  t.after(() => client.close());
  return { fake, client, emit: (event) => emit(event) };
}

for (const reported of [ERROR, VAULT, new URL(VAULT).origin]) {
  test(`reused probe session recovers ${reported} only by ordinary navigation`, async (t) => {
    const tab = errorTab(reported);
    const { fake, client } = await fixture(t, [tab]);
    await client.send('Target.getTargets');
    const { sessionId } = await client.send('Target.attachToTarget', {
      targetId: tab.targetId,
      flatten: true,
    });
    assert.equal(sessionId, tab.targetId);
    await assert.rejects(
      client.send('Runtime.evaluate', { expression: 'document.title' }, sessionId),
      /slicc: refused/
    );
    assert.equal(fake.calls.filter((call) => touching.has(call.method)).length, 0);
    await assert.rejects(client.send('Page.navigate', { url: VAULT }, sessionId), /slicc: refused/);
    await client.send('Page.navigate', { url: AFTER }, sessionId);
    await client.send(
      'Runtime.evaluate',
      { expression: 'document.title', returnByValue: true },
      sessionId
    );
    assert.ok(fake.calls.some((call) => call.method === 'Runtime.evaluate'));
    assert.deepEqual(
      fake.calls.filter((call) => call.method === 'Page.navigate').map((call) => call.params.url),
      [AFTER]
    );
    const probes = fake.calls.filter((call) => call.method === 'Target.detachFromTarget');
    assert.ok(probes.length >= 1);
    assert.ok(probes.every((call) => call.params.sessionId === tab.targetId));
  });
}

for (const attachEvents of [false, true]) {
  test(`reused blocked session cannot clear target protection, attach events ${attachEvents}`, async (t) => {
    const tab = { targetId: 'TAB', url: AFTER };
    const { fake, client, emit } = await fixture(t, [tab], { attachEvents });
    const attach = () => client.send('Target.attachToTarget', { targetId: 'TAB', flatten: true });
    const { sessionId } = await attach();
    tab.url = VAULT;
    emit({
      method: 'Page.frameNavigated',
      sessionId,
      params: {
        frame: { id: 'TAB', loaderId: 'P', url: VAULT, securityOrigin: new URL(VAULT).origin },
      },
    });
    await assert.rejects(
      client.send('Runtime.evaluate', { expression: 'document.title' }, sessionId),
      /slicc: refused/
    );
    await client.send('Target.detachFromTarget', { sessionId });
    await assert.rejects(attach(), /slicc: refused/);
    assert.equal(fake.calls.filter((call) => touching.has(call.method)).length, 0);
    tab.url = AFTER;
    emit({
      method: 'Page.frameNavigated',
      sessionId: 'TAB.1',
      params: {
        frame: { id: 'TAB', loaderId: 'A', url: AFTER, securityOrigin: new URL(AFTER).origin },
      },
    });
    const fresh = await attach();
    assert.equal(fresh.sessionId, sessionId);
    await client.send('Runtime.evaluate', { expression: 'document.title' }, fresh.sessionId);
  });
}

for (const reported of [VAULT, ERROR]) {
  test(`unproven ${reported} history and live vault documents stay refused`, async (t) => {
    const { fake, client } = await fixture(t, [
      errorTab(reported, VAULT),
      { targetId: 'LIVE', url: VAULT },
    ]);
    for (const targetId of ['1790572252', 'LIVE']) {
      await assert.rejects(
        client.send('Target.attachToTarget', { targetId, flatten: true }),
        /slicc: refused/
      );
    }
    assert.equal(
      fake.calls.filter((call) => touching.has(call.method) || call.method === 'Page.navigate')
        .length,
      0
    );
  });
}

function run(args, home) {
  return new Promise((resolve, reject) => {
    const env = {
      ...process.env,
      HOME: home,
      USERPROFILE: home,
      APPDATA: home,
      XDG_CONFIG_HOME: home,
      XDG_STATE_HOME: home,
    };
    delete env.SLICC_CDP_URL;
    delete env.PLAYWRIGHT_CLI_SESSION;
    const child = spawn(path.resolve('target/debug/playwright-cli'), args, { cwd: home, env });
    let stdout = '';
    let stderr = '';
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      reject(new Error('CLI timed out'));
    }, 30000);
    child.stdout.on('data', (chunk) => {
      stdout += chunk;
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
    });
    child.on('error', reject);
    child.on('close', (code) => {
      clearTimeout(timer);
      resolve({ code, stdout, stderr });
    });
  });
}

for (const reported of [ERROR, new URL(VAULT).origin]) {
  test(`CLI fresh connection recovers blocked-hop shape ${reported}`, async (t) => {
    const home = await mkdtemp(path.join(tmpdir(), 'slicc-session-reuse-'));
    const fake = await fakeCdp({ tabSessions: true });
    t.after(async () => {
      await fake.close();
      await rm(home, { recursive: true, force: true });
    });
    const pw = (...args) => run(['--cdp', fake.url, ...args], home);
    assert.equal((await pw('open', 'about:blank')).code, 0);
    Object.assign(fake.targets[0], errorTab(reported), { targetId: fake.targets[0].targetId });
    const after = await pw('goto', AFTER);
    assert.equal(after.code, 0, after.stderr);
    assert.equal((await pw('eval', 'document.title')).code, 0);
    assert.notEqual((await pw('goto', VAULT)).code, 0);
  });
}
