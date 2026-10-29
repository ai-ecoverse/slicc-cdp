import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';

const root = path.resolve(import.meta.dirname, '../..');
const native = path.join(root, 'target/debug');
const wasm = path.join(root, 'target/wasm32-wasip1/debug');
const binary = (name) => path.join(native, `${name}${process.platform === 'win32' ? '.exe' : ''}`);
const failed = 'https://seven.sliccy.ai/private?token=AWAY_SECRET#AWAY_SECRET';
const ordinary = 'https://ordinary.example/recovered';
const errorPage = 'chrome-error://chromewebdata/';
const errorFrame = {
  id: 'T',
  loaderId: 'ERROR',
  url: errorPage,
  securityOrigin: '://',
  unreachableUrl: failed,
  name: 'AWAY_SECRET',
};
const reads = [
  ['eval', 'document.body.textContent'],
  ['snapshot'],
  ['screenshot'],
  ['pdf'],
  ['localstorage-list'],
];

function clean(result) {
  assert.doesNotMatch(
    `${result.stdout}${result.stderr}`,
    /AWAY_SECRET|seven\.sliccy\.ai|chrome-error:/
  );
}

async function setup(transport, options = {}) {
  const backend = await fakeCdp({
    attachEvents: true,
    refuseWhen: (message) =>
      message.method === 'Page.getFrameTree' && backend.targets[0].frame?.url === errorPage,
    ...options,
  });
  backend.targets.push({
    targetId: 'T',
    url: failed,
    frame: { ...errorFrame },
    history: { currentIndex: 0, entries: [{ id: 1, url: errorPage, title: '' }] },
  });
  const runner =
    transport === 'native'
      ? nativeRunner(binary('playwright-cli'), binary('curlwright'), backend.url)
      : await kernelRunner(
          path.join(wasm, 'playwright-cli.wasm'),
          path.join(wasm, 'curlwright.wasm'),
          backend.url
        );
  const cwd = await runner.fresh();
  return { backend, runner, cwd };
}

for (const transport of ['native', 'kernel']) {
  test(`navigate away waits for a new document before observing it (${transport})`, {
    timeout: 120_000,
  }, async () => {
    let release;
    let requested;
    const pending = new Promise((resolve) => {
      requested = resolve;
    });
    const fixture = await setup(transport, {
      intercept(message, send) {
        if (message.method !== 'Page.navigate') return false;
        const sessionId = message.sessionId;
        send({
          sessionId,
          method: 'Runtime.consoleAPICalled',
          params: { args: [{ type: 'string', value: 'AWAY_SECRET' }] },
        });
        send({ sessionId, method: 'Page.frameNavigated', params: { frame: errorFrame } });
        release = () => {
          const frame = {
            id: 'T',
            loaderId: 'NEW',
            url: ordinary,
            securityOrigin: 'https://ordinary.example',
          };
          Object.assign(fixture.backend.targets[0], { url: ordinary, frame, loaderId: 'NEW' });
          delete fixture.backend.targets[0].history;
          send({ id: message.id, result: { frameId: 'T', loaderId: 'NEW' } });
          send({ sessionId, method: 'Page.frameNavigated', params: { frame } });
          send({
            sessionId,
            method: 'DOM.documentUpdated',
            params: { text: 'AWAY_SECRET', url: failed, title: 'AWAY_SECRET' },
          });
          send({ sessionId, method: 'Page.loadEventFired', params: {} });
          send({
            sessionId,
            method: 'Runtime.consoleAPICalled',
            params: { args: [{ type: 'string', value: 'AWAY_SECRET' }] },
          });
        };
        requested();
        return true;
      },
    });
    const { backend, runner, cwd } = fixture;
    const pw = (args) => runner.playwright([...args, '--tab', 'T'], cwd);
    let going;
    try {
      going = pw(['goto', ' \thttps://ORDINARY.example/recovered']);
      await Promise.race([
        pending,
        going.then((result) => assert.equal(result.code, 0, result.stderr)),
      ]);
      assert.equal(
        backend.calls.find((call) => call.method === 'Page.navigate').params.url,
        ordinary
      );
      assert.ok(
        backend.calls.every((call) =>
          [
            'Target.getTargetInfo',
            'Target.attachToTarget',
            'Page.enable',
            'Page.navigate',
          ].includes(call.method)
        ),
        'the navigation session observed the old document'
      );
      for (const args of reads) {
        const result = await pw(args);
        assert.notEqual(result.code, 0, args[0]);
        clean(result);
      }
      assert.ok(
        !backend.calls.some((call) =>
          /^(Runtime\.|DOM\.|Accessibility\.|DOMSnapshot\.|Network\.|Fetch\.|Tracing\.)/.test(
            call.method
          )
        )
      );
      release();
      const result = await going;
      assert.equal(result.code, 0, result.stderr);
      clean(result);
      assert.equal(backend.targets[0].url, ordinary);
      assert.ok(backend.calls.some((call) => call.method === 'Fetch.enable'));
      const content = await pw(['eval', 'document.body.textContent']);
      assert.equal(content.code, 0, content.stderr);
      assert.match(content.stdout, /fake-eval/);
      clean(content);
      clean(await pw(['console']));
    } finally {
      release?.();
      await going?.catch(() => {});
      await runner.close();
      await backend.close();
    }
  });

  test(`navigate away refuses unsafe destinations (${transport})`, {
    timeout: 60_000,
  }, async () => {
    const { backend, runner, cwd } = await setup(transport);
    try {
      for (const url of [failed, ' \thttps://SEVEN.sliccy.ai./private', 'https://', 'null']) {
        const from = backend.calls.length;
        const result = await runner.playwright(['navigate', url, '--tab', 'T'], cwd);
        assert.notEqual(result.code, 0, url);
        clean(result);
        assert.ok(
          backend.calls
            .slice(from)
            .every((call) =>
              [
                'Slicc.action',
                'Target.getTargets',
                'Target.getTargetInfo',
                'Target.attachToTarget',
                'Target.detachFromTarget',
                'Page.getFrameTree',
              ].includes(call.method)
            ),
          url
        );
      }
      assert.equal(backend.targets[0].url, failed);
    } finally {
      await runner.close();
      await backend.close();
    }
  });

  for (const ending of ['error', 'protected', 'unknown', 'failure', 'download']) {
    test(`navigate away leaves a ${ending} outcome unreadable (${transport})`, {
      timeout: 60_000,
    }, async () => {
      const { backend, runner, cwd } = await setup(transport, {
        intercept(message, send) {
          if (message.method !== 'Page.navigate') return false;
          const tab = backend.targets[0];
          const url = ending === 'protected' ? failed : ending === 'unknown' ? 'null' : errorPage;
          const frame = {
            ...errorFrame,
            loaderId: 'NEW',
            url,
            securityOrigin: ending === 'protected' ? 'https://seven.sliccy.ai' : '://',
            ...(ending === 'protected' ? { unreachableUrl: '' } : {}),
          };
          Object.assign(tab, { url: ending === 'error' ? failed : url, frame });
          const result = {
            frameId: 'T',
            loaderId: 'NEW',
            ...(ending === 'failure' ? { errorText: `AWAY_SECRET ${failed}` } : {}),
            ...(ending === 'download' ? { isDownload: true } : {}),
          };
          send({ id: message.id, result });
          send({ sessionId: message.sessionId, method: 'Page.frameNavigated', params: { frame } });
          send({ sessionId: message.sessionId, method: 'Page.loadEventFired', params: {} });
          return true;
        },
      });
      try {
        const result = await runner.playwright(['goto', ordinary, '--tab', 'T'], cwd);
        assert.notEqual(result.code, 0);
        clean(result);
        assert.ok(
          !backend.calls.some((call) =>
            /^(Runtime\.|DOM\.|Accessibility\.|DOMSnapshot\.|Network\.|Fetch\.|Tracing\.)/.test(
              call.method
            )
          )
        );
        const read = await runner.playwright(
          ['eval', 'document.body.textContent', '--tab', 'T'],
          cwd
        );
        assert.notEqual(read.code, 0);
        clean(read);
      } finally {
        await runner.close();
        await backend.close();
      }
    });
  }
}
