import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';

const root = path.resolve(import.meta.dirname, '../..');
const native = path.join(root, 'target/debug');
const wasm = path.join(root, 'target/wasm32-wasip1/debug');
const binary = (name) => path.join(native, `${name}${process.platform === 'win32' ? '.exe' : ''}`);
const failed = 'https://seven.sliccy.ai/private?token=AWAY_SECRET#AWAY_SECRET';
const ordinary = 'https://ordinary.example/recovered';
const errorPage = 'chrome-error://chromewebdata/';
const biosRefusal =
  'slicc: refused: seven’s own pages and the vault are out of reach of browser automation';
const sevenRefusal = "slicc: refused: target is on seven's origin\n";
const stopped = (step) => `${sevenRefusal}slicc: navigate away stopped at: ${step}\n`;
const failure = { error: { message: 'AWAY_SECRET' } };
const standIn = { frameTree: { frame: { id: '0', url: errorPage, securityOrigin: '://' } } };
const errorFrame = {
  id: 'T',
  loaderId: 'ERROR',
  url: errorPage,
  securityOrigin: '://',
  unreachableUrl: failed,
  name: 'AWAY_SECRET',
};
const committedFrame = (url = ordinary, loaderId = 'NEW') => ({
  id: 'T',
  loaderId,
  url,
  securityOrigin: url === ordinary ? 'https://ordinary.example' : 'https://seven.sliccy.ai',
});
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

function unobserved(calls) {
  assert.ok(
    !calls.some((call) =>
      /^(Runtime\.|DOM\.|Accessibility\.|DOMSnapshot\.|Network\.|Tracing\.)/.test(call.method)
    )
  );
  for (const call of calls.filter((call) => call.method === 'Fetch.enable')) {
    assert.deepEqual(call.params, {
      patterns: [{ urlPattern: '*', resourceType: 'Document', requestStage: 'Request' }],
    });
  }
  assert.ok(
    calls
      .filter((call) => call.method.startsWith('Fetch.'))
      .every((call) =>
        ['Fetch.enable', 'Fetch.continueRequest', 'Fetch.failRequest'].includes(call.method)
      )
  );
}

async function setup(transport, options = {}) {
  const { gate = 'bios-refusal', intercept, onCommand, ...rest } = options;
  const backend = await fakeCdp({
    attachEvents: true,
    refusalMessage: biosRefusal,
    refuseWhen: (message) =>
      gate === 'bios-refusal' &&
      message.method === 'Page.getFrameTree' &&
      backend.targets[0].frame?.url === errorPage,
    intercept(message, send, close) {
      if (
        gate === 'bios-tree' &&
        message.method === 'Page.getFrameTree' &&
        backend.targets[0].frame?.url === errorPage
      ) {
        send({ id: message.id, result: standIn });
        return true;
      }
      return intercept?.(message, send, close) ?? false;
    },
    onCommand(message, event) {
      if (message.method === 'Page.enable' && backend.targets[0].frame?.url === errorPage)
        event('Page.frameNavigated', { frame: errorFrame, type: 'Navigation' });
      onCommand?.(message, event);
    },
    ...rest,
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

function documentRequest(send, sessionId, url, requestId = 'NAV') {
  send({
    sessionId,
    method: 'Fetch.requestPaused',
    params: {
      requestId,
      frameId: 'T',
      resourceType: 'Document',
      request: { url, method: 'GET', headers: {} },
    },
  });
}

function finish(send, message, frame = committedFrame(), type = 'Navigation') {
  send({ id: message.id, result: { frameId: 'T', loaderId: frame.loaderId } });
  send({ sessionId: message.sessionId, method: 'Page.frameNavigated', params: { frame, type } });
  send({ sessionId: message.sessionId, method: 'Page.loadEventFired', params: {} });
}

for (const transport of ['native', 'kernel']) {
  for (const gate of ['bios-refusal', 'bios-tree', 'direct']) {
    test(`navigate away verifies a new document through ${gate} (${transport})`, {
      timeout: 120_000,
    }, async () => {
      let release;
      let requested;
      const pending = new Promise((resolve) => {
        requested = resolve;
      });
      const fixture = await setup(transport, {
        gate,
        intercept(message, send) {
          if (message.method !== 'Page.navigate') return false;
          if (gate === 'direct') documentRequest(send, message.sessionId, ordinary);
          send({
            sessionId: message.sessionId,
            method: 'Runtime.consoleAPICalled',
            params: { args: [{ type: 'string', value: 'AWAY_SECRET' }] },
          });
          release = () => {
            const frame = committedFrame();
            Object.assign(fixture.backend.targets[0], { url: ordinary, frame, loaderId: 'NEW' });
            delete fixture.backend.targets[0].history;
            finish(send, message);
            send({
              sessionId: message.sessionId,
              method: 'DOM.documentUpdated',
              params: { text: 'AWAY_SECRET', url: failed, title: 'AWAY_SECRET' },
            });
            send({
              sessionId: message.sessionId,
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
        const navigation = backend.calls.find((call) => call.method === 'Page.navigate');
        assert.equal(navigation.params.url, ordinary);
        const session = navigation.sessionId;
        const own = backend.calls.filter((call) => call.sessionId === session);
        assert.ok(
          own.findIndex((call) => call.method === 'Page.getFrameTree') <
            own.findIndex((call) => call.method === 'Page.enable')
        );
        assert.ok(own.some((call) => call.method === 'Page.getFrameTree'));
        assert.equal(
          own.some((call) => call.method === 'Fetch.enable'),
          gate === 'direct'
        );
        unobserved(backend.calls);
        for (const args of reads) {
          const result = await pw(args);
          assert.notEqual(result.code, 0, args[0]);
          clean(result);
        }
        unobserved(backend.calls);
        release();
        const result = await going;
        assert.equal(result.code, 0, result.stderr);
        clean(result);
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
  }

  test(`navigate away refuses unsafe destinations before connecting (${transport})`, {
    timeout: 60_000,
  }, async () => {
    const { backend, runner, cwd } = await setup(transport);
    try {
      for (const url of [failed, ' \thttps://SEVEN.sliccy.ai./private', 'https://']) {
        const from = backend.calls.length;
        const result = await runner.playwright(['navigate', url, '--tab', 'T'], cwd);
        assert.notEqual(result.code, 0, url);
        clean(result);
        assert.equal(backend.calls.length, from, url);
      }
    } finally {
      await runner.close();
      await backend.close();
    }
  });

  test(`navigate away leaves non-HTTP destinations on the refused base path (${transport})`, {
    timeout: 60_000,
  }, async () => {
    const { backend, runner, cwd } = await setup(transport);
    try {
      for (const url of [
        'null',
        'javascript:document.title',
        'data:text/html,new',
        'about:blank',
      ]) {
        const from = backend.calls.length;
        const result = await runner.playwright(['navigate', url, '--tab', 'T'], cwd);
        assert.notEqual(result.code, 0, url);
        clean(result);
        assert.ok(
          !backend.calls
            .slice(from)
            .some((call) => ['Page.navigate', 'Page.enable', 'Fetch.enable'].includes(call.method)),
          url
        );
      }
      unobserved(backend.calls);
    } finally {
      await runner.close();
      await backend.close();
    }
  });

  for (const refusal of [
    "slicc: refused: target is on seven's origin",
    `${biosRefusal} extra`,
    'Page.getFrameTree refused',
  ]) {
    test(`navigate away does not delegate a different refusal ${refusal.length} (${transport})`, {
      timeout: 60_000,
    }, async () => {
      const { backend, runner, cwd } = await setup(transport, { refusalMessage: refusal });
      try {
        const result = await runner.playwright(['goto', ordinary, '--tab', 'T'], cwd);
        assert.notEqual(result.code, 0);
        clean(result);
        assert.ok(
          !backend.calls.some((call) =>
            ['Page.enable', 'Page.navigate', 'Fetch.enable'].includes(call.method)
          )
        );
      } finally {
        await runner.close();
        await backend.close();
      }
    });
  }

  for (const [miss, tree] of [
    [
      'unreachable',
      { frameTree: { frame: { ...standIn.frameTree.frame, unreachableUrl: failed } } },
    ],
    ['name', { frameTree: { frame: { ...standIn.frameTree.frame, name: 'AWAY_SECRET' } } }],
    ['children', { frameTree: { ...standIn.frameTree, childFrames: [] } }],
    ['url', { frameTree: { frame: { ...standIn.frameTree.frame, url: ordinary } } }],
    ['origin', { frameTree: { frame: { ...standIn.frameTree.frame, securityOrigin: 'null' } } }],
    ['outer-field', { ...standIn, other: true }],
  ]) {
    test(`navigate away rejects a stand-in near miss ${miss} (${transport})`, {
      timeout: 60_000,
    }, async () => {
      const { backend, runner, cwd } = await setup(transport, {
        gate: 'direct',
        intercept(message, send) {
          if (message.method !== 'Page.getFrameTree') return false;
          send({ id: message.id, result: tree });
          return true;
        },
      });
      try {
        const result = await runner.playwright(['goto', ordinary, '--tab', 'T'], cwd);
        assert.notEqual(result.code, 0);
        clean(result);
        assert.ok(
          !backend.calls.some((call) =>
            ['Page.enable', 'Page.navigate', 'Fetch.enable'].includes(call.method)
          )
        );
        assert.equal(backend.targets[0].url, failed);
      } finally {
        await runner.close();
        await backend.close();
      }
    });
  }

  for (const source of ['live', 'stand-in-like']) {
    test(`navigate away refuses a readable ${source} tree (${transport})`, {
      timeout: 60_000,
    }, async () => {
      const { backend, runner, cwd } = await setup(transport, { gate: 'direct' });
      backend.targets[0].frame =
        source === 'live'
          ? { ...committedFrame(failed), loaderId: 'OLD' }
          : { id: 'T', url: errorPage, securityOrigin: '://' };
      try {
        const result = await runner.playwright(['goto', ordinary, '--tab', 'T'], cwd);
        assert.notEqual(result.code, 0);
        clean(result);
        assert.ok(
          !backend.calls.some((call) =>
            ['Page.enable', 'Page.navigate', 'Fetch.enable'].includes(call.method)
          )
        );
        assert.equal(backend.targets[0].url, failed);
      } finally {
        await runner.close();
        await backend.close();
      }
    });
  }

  test(`navigate away treats the front guard's refusal as final (${transport})`, {
    timeout: 60_000,
  }, async () => {
    const { backend, runner, cwd } = await setup(transport, {
      gate: 'bios-tree',
      intercept(message, send) {
        if (message.method !== 'Page.navigate') return false;
        send({ id: message.id, error: { message: 'AWAY_SECRET' } });
        return true;
      },
    });
    try {
      const result = await runner.playwright(['goto', ordinary, '--tab', 'T'], cwd);
      assert.notEqual(result.code, 0);
      clean(result);
      assert.equal(backend.calls.filter((call) => call.method === 'Page.navigate').length, 1);
      unobserved(backend.calls);
      const content = await runner.playwright(
        ['eval', 'document.body.textContent', '--tab', 'T'],
        cwd
      );
      assert.notEqual(content.code, 0);
      clean(content);
    } finally {
      await runner.close();
      await backend.close();
    }
  });

  test(`navigate away never returns a protocol error's old title (${transport})`, {
    timeout: 60_000,
  }, async () => {
    const { backend, runner, cwd } = await setup(transport, {
      intercept(message, send) {
        if (message.method !== 'Page.enable') return false;
        send({ id: message.id, error: { message: 'AWAY_SECRET' } });
        return true;
      },
    });
    try {
      const result = await runner.playwright(['goto', ordinary, '--tab', 'T'], cwd);
      assert.notEqual(result.code, 0);
      clean(result);
      assert.ok(!backend.calls.some((call) => call.method === 'Page.navigate'));
    } finally {
      await runner.close();
      await backend.close();
    }
  });

  for (const [miss, step, gate, method, answer] of [
    ['attach failure', 'target', 'direct', 'Target.attachToTarget', failure],
    ['session', 'target', 'direct', 'Target.attachToTarget', { result: {} }],
    ['tree failure', 'target', 'direct', 'Page.getFrameTree', failure],
    [
      'readable tree',
      'target',
      'direct',
      'Page.getFrameTree',
      { result: { frameTree: { frame: committedFrame(failed, 'OLD') } } },
    ],
    ['page enable', 'enable', 'bios-tree', 'Page.enable', failure],
    ['fetch enable', 'enable', 'direct', 'Fetch.enable', failure],
    ['navigate failure', 'navigate', 'bios-tree', 'Page.navigate', failure],
    ['error text', 'navigate', 'bios-tree', 'Page.navigate', { result: { errorText: 'x' } }],
    ['download', 'navigate', 'bios-tree', 'Page.navigate', { result: { isDownload: true } }],
    ['frame', 'navigate', 'bios-tree', 'Page.navigate', { result: { loaderId: 'NEW' } }],
    ['loader', 'navigate', 'bios-tree', 'Page.navigate', { result: { frameId: 'T' } }],
  ]) {
    test(`navigate away names ${step} on ${miss} (${transport})`, {
      timeout: 60_000,
    }, async () => {
      const { backend, runner, cwd } = await setup(transport, {
        gate,
        intercept(message, send) {
          if (message.method !== method) return false;
          send({ id: message.id, ...answer });
          return true;
        },
      });
      try {
        const result = await runner.playwright(['goto', ordinary, '--tab', 'T'], cwd);
        assert.notEqual(result.code, 0);
        assert.equal(result.stdout, '');
        assert.equal(result.stderr, stopped(step));
        clean(result);
      } finally {
        await runner.close();
        await backend.close();
      }
    });
  }

  test(`navigate away names enable on a commit during enable (${transport})`, {
    timeout: 60_000,
  }, async () => {
    const { backend, runner, cwd } = await setup(transport, {
      gate: 'direct',
      onCommand(message, event) {
        if (message.method === 'Page.enable')
          event('Page.frameNavigated', {
            frame: committedFrame(ordinary, 'LATE'),
            type: 'Navigation',
          });
      },
    });
    try {
      const result = await runner.playwright(['goto', ordinary, '--tab', 'T'], cwd);
      assert.notEqual(result.code, 0);
      assert.equal(result.stderr, stopped('enable'));
    } finally {
      await runner.close();
      await backend.close();
    }
  });

  for (const [state, step] of [
    ['live', 'target'],
    ['error', 'enable'],
  ]) {
    for (const [owner, url, securityOrigin] of [
      ['seven', failed, 'https://seven.sliccy.ai'],
      ['the vault', 'https://vault.sliccy.ai/private', 'https://vault.sliccy.ai'],
      ['unknown', 'https://', 'null'],
    ]) {
      test(`navigate away says the same for a ${state} tab of ${owner} (${transport})`, {
        timeout: 60_000,
      }, async () => {
        const { backend, runner, cwd } = await setup(transport, {
          gate: 'direct',
          intercept(message, send) {
            if (message.method !== 'Page.enable') return false;
            send({ id: message.id, ...failure });
            return true;
          },
        });
        Object.assign(backend.targets[0], {
          url,
          frame:
            state === 'live'
              ? { id: 'T', loaderId: 'OLD', url, securityOrigin }
              : { ...errorFrame, unreachableUrl: url },
        });
        try {
          const result = await runner.playwright(['goto', ordinary, '--tab', 'T'], cwd);
          assert.notEqual(result.code, 0);
          assert.equal(result.stderr, stopped(step));
          clean(result);
        } finally {
          await runner.close();
          await backend.close();
        }
      });
    }
  }

  test(`navigate away names no step after the navigate reply (${transport})`, {
    timeout: 60_000,
  }, async () => {
    const { backend, runner, cwd } = await setup(transport, {
      intercept(message, send) {
        if (message.method !== 'Page.navigate') return false;
        const frame = committedFrame(failed);
        Object.assign(backend.targets[0], { frame });
        delete backend.targets[0].history;
        finish(send, message, frame);
        return true;
      },
    });
    try {
      const result = await runner.playwright(['goto', ordinary, '--tab', 'T'], cwd);
      assert.notEqual(result.code, 0);
      assert.equal(result.stderr, sevenRefusal);
    } finally {
      await runner.close();
      await backend.close();
    }
  });

  for (const ending of [
    'error',
    'protected',
    'unknown',
    'failure',
    'download',
    'BackForwardCacheRestore',
    'PrerenderActivation',
    'stale-loader',
  ]) {
    test(`navigate away rejects a ${ending} outcome (${transport})`, {
      timeout: 60_000,
    }, async () => {
      const { backend, runner, cwd } = await setup(transport, {
        intercept(message, send) {
          if (message.method !== 'Page.navigate') return false;
          const url =
            ending === 'protected'
              ? failed
              : ending === 'unknown'
                ? 'null'
                : ending === 'error'
                  ? errorPage
                  : ordinary;
          const frame = {
            ...committedFrame(),
            url,
            ...(ending === 'error' ? { securityOrigin: '://', unreachableUrl: failed } : {}),
            ...(ending === 'stale-loader' ? { loaderId: 'ERROR' } : {}),
          };
          if (!['failure', 'download'].includes(ending)) {
            Object.assign(backend.targets[0], { url: ending === 'error' ? failed : url, frame });
            delete backend.targets[0].history;
          }
          if (ending === 'failure' || ending === 'download')
            send({
              id: message.id,
              result: {
                frameId: 'T',
                loaderId: 'NEW',
                ...(ending === 'failure' ? { errorText: 'AWAY_SECRET' } : { isDownload: true }),
              },
            });
          else
            finish(
              send,
              message,
              frame,
              ['BackForwardCacheRestore', 'PrerenderActivation'].includes(ending)
                ? ending
                : 'Navigation'
            );
          return true;
        },
      });
      try {
        const result = await runner.playwright(['goto', ordinary, '--tab', 'T'], cwd);
        assert.notEqual(result.code, 0);
        clean(result);
        unobserved(backend.calls);
      } finally {
        await runner.close();
        await backend.close();
      }
    });
  }

  for (const hop of ['request', 'network', 'commit']) {
    test(`navigate away latches an intermediate protected ${hop} (${transport})`, {
      timeout: 60_000,
    }, async () => {
      const { backend, runner, cwd } = await setup(transport, {
        gate: 'direct',
        intercept(message, send) {
          if (message.method !== 'Page.navigate') return false;
          documentRequest(send, message.sessionId, ordinary, 'FIRST');
          if (hop === 'request') {
            documentRequest(send, message.sessionId, failed, 'SECOND');
            documentRequest(send, message.sessionId, ordinary, 'THIRD');
          } else if (hop === 'network') {
            send({
              sessionId: message.sessionId,
              method: 'Network.requestWillBeSent',
              params: {
                type: 'Document',
                frameId: 'T',
                request: { url: ordinary },
                redirectResponse: { url: failed },
              },
            });
          } else {
            send({
              sessionId: message.sessionId,
              method: 'Page.frameNavigated',
              params: { frame: committedFrame(failed), type: 'Navigation' },
            });
          }
          const frame = committedFrame();
          Object.assign(backend.targets[0], { url: ordinary, frame });
          delete backend.targets[0].history;
          finish(send, message);
          return true;
        },
      });
      try {
        const result = await runner.playwright(['goto', ordinary, '--tab', 'T'], cwd);
        assert.notEqual(result.code, 0);
        clean(result);
        unobserved(backend.calls);
        if (hop === 'request')
          assert.ok(
            backend.calls.some(
              (call) => call.method === 'Fetch.failRequest' && call.params.requestId === 'SECOND'
            )
          );
      } finally {
        await runner.close();
        await backend.close();
      }
    });
  }

  for (const late of ['protected-restore', 'restore', 'redirect', 'request', 'commit']) {
    test(`navigate away rejects a late ${late} during detach (${transport})`, {
      timeout: 60_000,
    }, async () => {
      let navigationSession;
      const fixture = await setup(transport, {
        gate: 'direct',
        intercept(message, send) {
          if (message.method === 'Page.navigate') {
            navigationSession = message.sessionId;
            documentRequest(send, navigationSession, ordinary);
            const frame = committedFrame();
            Object.assign(fixture.backend.targets[0], { url: ordinary, frame });
            delete fixture.backend.targets[0].history;
            finish(send, message);
            return true;
          }
          if (
            message.method !== 'Target.detachFromTarget' ||
            message.params.sessionId !== navigationSession
          )
            return false;
          if (late === 'request') documentRequest(send, navigationSession, failed, 'LATE');
          else if (late === 'redirect')
            send({
              sessionId: navigationSession,
              method: 'Network.requestWillBeSent',
              params: {
                type: 'Document',
                frameId: 'T',
                request: { url: ordinary },
                redirectResponse: { url: failed },
              },
            });
          else {
            if (late === 'protected-restore')
              send({
                sessionId: navigationSession,
                method: 'Page.frameNavigated',
                params: { frame: committedFrame(failed, 'HOP'), type: 'BackForwardCacheRestore' },
              });
            send({
              sessionId: navigationSession,
              method: 'Page.frameNavigated',
              params: {
                frame: committedFrame(),
                type: late === 'commit' ? 'Navigation' : 'BackForwardCacheRestore',
              },
            });
          }
          return false;
        },
      });
      const { backend, runner, cwd } = fixture;
      try {
        const result = await runner.playwright(['goto', ordinary, '--tab', 'T'], cwd);
        assert.notEqual(result.code, 0);
        clean(result);
        unobserved(backend.calls);
        if (late === 'request')
          assert.ok(
            backend.calls.some(
              (call) => call.method === 'Fetch.failRequest' && call.params.requestId === 'LATE'
            )
          );
        assert.equal(
          backend.calls.filter((call) => call.method === 'Target.attachToTarget').length,
          1
        );
      } finally {
        await runner.close();
        await backend.close();
      }
    });
  }

  for (const late of [
    'protected-restore',
    'restore',
    'redirect',
    'request',
    'commit',
    'ordinary-event',
  ]) {
    test(`navigation verdict rejects ${late} after the scope proof (${transport})`, {
      timeout: 60_000,
    }, async () => {
      let navigationSession;
      let scopeInventory = false;
      let injected = false;
      const fixture = await setup(transport, {
        gate: 'direct',
        onCommand(message) {
          if (message.method === 'Target.getTargets' && message.params.filter)
            scopeInventory = true;
        },
        intercept(message, send) {
          if (message.method === 'Page.navigate') {
            navigationSession = message.sessionId;
            documentRequest(send, navigationSession, ordinary);
            Object.assign(fixture.backend.targets[0], { url: ordinary, frame: committedFrame() });
            delete fixture.backend.targets[0].history;
            finish(send, message);
            return true;
          }
          if (message.method !== 'Page.getFrameTree' || !scopeInventory || injected) return false;
          injected = true;
          send({ id: message.id, result: { frameTree: { frame: committedFrame() } } });
          if (late === 'request') documentRequest(send, navigationSession, failed, 'LATE');
          else if (late === 'redirect')
            send({
              sessionId: navigationSession,
              method: 'Network.requestWillBeSent',
              params: {
                type: 'Document',
                frameId: 'T',
                request: { url: ordinary },
                redirectResponse: { url: failed },
              },
            });
          else if (late === 'ordinary-event')
            send({ sessionId: navigationSession, method: 'Page.loadEventFired', params: {} });
          else {
            if (late === 'protected-restore')
              send({
                sessionId: navigationSession,
                method: 'Page.frameNavigated',
                params: { frame: committedFrame(failed, 'HOP'), type: 'BackForwardCacheRestore' },
              });
            send({
              sessionId: navigationSession,
              method: 'Page.frameNavigated',
              params: {
                frame: committedFrame(),
                type: late === 'commit' ? 'Navigation' : 'BackForwardCacheRestore',
              },
            });
          }
          return true;
        },
      });
      const { backend, runner, cwd } = fixture;
      try {
        const result = await runner.playwright(['goto', ordinary, '--tab', 'T'], cwd);
        assert.ok(injected);
        assert.notEqual(result.code, 0);
        assert.equal(result.stdout, '');
        clean(result);
        assert.match(result.stderr, /^slicc: refused:/);
        if (late === 'request')
          assert.ok(
            backend.calls.some(
              (call) => call.method === 'Fetch.failRequest' && call.params.requestId === 'LATE'
            )
          );
        const listed = await runner.playwright(['tab-list'], cwd);
        assert.equal(listed.code, 0, listed.stderr);
        assert.doesNotMatch(listed.stdout, /\(active\)/);
      } finally {
        await runner.close();
        await backend.close();
      }
    });
  }

  for (const late of ['away-event', 'normal-commit']) {
    test(`shutdown verdict rejects ${late} before output (${transport})`, {
      timeout: 60_000,
    }, async () => {
      let navigationSession;
      let proofSession;
      let scopeInventory = false;
      let injected = false;
      const fixture = await setup(transport, {
        gate: 'direct',
        onCommand(message) {
          if (message.method === 'Target.getTargets' && message.params.filter)
            scopeInventory = true;
          if (scopeInventory && message.method === 'Page.getFrameTree')
            proofSession = message.sessionId;
        },
        intercept(message, send) {
          if (message.method === 'Page.navigate') {
            navigationSession = message.sessionId;
            documentRequest(send, navigationSession, ordinary);
            Object.assign(fixture.backend.targets[0], { url: ordinary, frame: committedFrame() });
            delete fixture.backend.targets[0].history;
            finish(send, message);
            return true;
          }
          if (
            message.method === 'Fetch.disable' &&
            message.sessionId === proofSession &&
            !injected
          ) {
            injected = true;
            send(
              late === 'away-event'
                ? { sessionId: navigationSession, method: 'Page.loadEventFired', params: {} }
                : {
                    sessionId: proofSession,
                    method: 'Page.frameNavigated',
                    params: { frame: committedFrame(), type: 'Navigation' },
                  }
            );
          }
          return false;
        },
      });
      const { runner, cwd } = fixture;
      try {
        const result = await runner.playwright(['goto', ordinary, '--tab', 'T'], cwd);
        assert.ok(injected);
        assert.notEqual(result.code, 0);
        assert.equal(result.stdout, '');
        assert.match(result.stderr, /^slicc: refused:/);
        clean(result);
        const listed = await runner.playwright(['tab-list'], cwd);
        assert.equal(listed.code, 0, listed.stderr);
        assert.doesNotMatch(listed.stdout, /\(active\)/);
      } finally {
        await runner.close();
        await fixture.backend.close();
      }
    });
  }

  test(`history after recovery does not expose an old protected title (${transport})`, {
    timeout: 60_000,
  }, async () => {
    const { backend, runner, cwd } = await setup(transport, {
      intercept(message, send) {
        if (message.method !== 'Page.navigate') return false;
        const frame = committedFrame();
        Object.assign(backend.targets[0], {
          url: ordinary,
          frame,
          history: {
            currentIndex: 1,
            entries: [
              { id: 1, url: failed, title: 'AWAY_SECRET' },
              { id: 2, url: ordinary, title: 'Ordinary' },
            ],
          },
        });
        finish(send, message);
        return true;
      },
    });
    let client;
    try {
      const result = await runner.playwright(['goto', ordinary, '--tab', 'T'], cwd);
      assert.equal(result.code, 0, result.stderr);
      clean(result);
      client = await guarded(backend.url, 'https://private.example.invalid', {
        variant: 'prompt',
        detachDelay: 0,
        transport: transport === 'kernel' ? 'wasi' : 'native',
      });
      const { sessionId } = await client.send('Target.attachToTarget', {
        targetId: 'T',
        flatten: true,
      });
      const history = await client.send('Page.getNavigationHistory', {}, sessionId);
      assert.doesNotMatch(JSON.stringify(history), /AWAY_SECRET/);
      assert.ok(['', 'https://seven.sliccy.ai'].includes(history.entries[0].title));
      assert.equal(history.entries[1].title, 'Ordinary');
    } finally {
      await client?.close();
      await runner.close();
      await backend.close();
    }
  });
}
