import assert from 'node:assert/strict';
import { registerHooks } from 'node:module';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { ForwardLedger } from './forward-ledger.mjs';
import { guarded } from './guard-client.mjs';

const published = new URL(
  './bfcache-restore.mjs',
  import.meta.resolve('@ai-ecoverse/slicc-browsers/package.json')
).href;
const hook = registerHooks({
  load(url, context, nextLoad) {
    const loaded = nextLoad(url, context);
    return url === published
      ? { ...loaded, source: `${loaded.source}\nexport { Restore };\n` }
      : loaded;
  },
});
const { Restore } = await import(published);
hook.deregister();

for (const transport of ['native', 'wasi']) {
  test(`published final audit supersedes a real pending early snapshot through ${transport}`, {
    timeout: 60000,
  }, async () => {
    const frame = {
      id: 'TAB',
      loaderId: 'L1',
      url: 'https://ordinary.example/',
      securityOrigin: 'https://ordinary.example',
    };
    let armed = false;
    let release;
    let enter;
    const entered = new Promise((resolve) => {
      enter = resolve;
    });
    const backend = await fakeCdp({
      intercept: (message, send) => {
        if (armed && message.method === 'Page.getFrameTree') {
          armed = false;
          release = () =>
            send({
              id: message.id,
              sessionId: message.sessionId,
              result: { frameTree: { frame } },
            });
          enter();
          return true;
        }
        return false;
      },
    });
    backend.targets.push({ targetId: 'TAB', url: frame.url, frame });
    const ledger = new ForwardLedger();
    let client;
    try {
      client = await guarded(backend.url, 'https://private.example.invalid', {
        transport,
        variant: 'prompt',
        detachDelay: 0,
        onCommand: (message) => ledger.request(message),
        onMessage: (message) => ledger.message(message),
      });
      await ledger.initialized(5000);
      const session = (
        await client.send('Target.attachToTarget', { targetId: 'TAB', flatten: true })
      ).sessionId;
      armed = true;
      const read = client.send('DOM.getDocument', { depth: -1, pierce: true }, session);
      await entered;
      const params = { source: 'persistent capability' };
      const sentAt = Date.now();
      const denied = assert.rejects(
        client.send('Page.addScriptToEvaluateOnNewDocument', params),
        /slicc: refused: persistent page capabilities/
      );
      const restore = new Restore({ forwardAudit: true, tag: 'audit-contract' });
      restore.result.probes = [
        { id: 1, method: 'Page.addScriptToEvaluateOnNewDocument', params, sentAt },
      ];
      restore.gate = { audit: ledger.auditor(undefined) };
      await restore.audit(false);
      assert.equal(restore.result.delay.audit.result.complete, false);
      assert.equal(restore.result.delay.audit.result.probes[0].complete, false);
      assert.equal(restore.result.commandsUnforwarded, 'incomplete');
      assert.deepEqual(restore.missing, []);
      assert(
        !backend.calls.some((message) => message.method === 'Page.addScriptToEvaluateOnNewDocument')
      );
      release();
      await read;
      await denied;
      await restore.audit(true);
      assert.equal(restore.result.delay.finalAudit.result.complete, true);
      assert.equal(restore.result.delay.finalAudit.result.probes[0].complete, true);
      assert.equal(restore.result.delay.finalAudit.result.probes[0].forwarded, false);
      assert.equal(restore.result.commandsUnforwarded, true);
      assert.equal(restore.finalAuditComplete, true);
      assert.deepEqual(restore.missing, []);
    } finally {
      release?.();
      await client?.close();
      await backend.close();
    }
  });
}

test('published incomplete terminal audit remains missing evidence', async () => {
  const ledger = new ForwardLedger();
  ledger.message({ audit: { stage: 'ready', version: 1 } });
  const params = { expression: 'document.title' };
  ledger.request({ id: 1, method: 'Runtime.evaluate', params, sessionId: 'S' });
  ledger.message({ audit: { id: 1, stage: 'received', method: 'Runtime.evaluate' } });
  ledger.wait = async () => {};
  const restore = new Restore({ forwardAudit: true, tag: 'terminal-contract' });
  restore.result.probes = [{ id: 1, method: 'Runtime.evaluate', params, sentAt: 0 }];
  restore.gate = { audit: ledger.auditor('S') };
  await restore.audit(false);
  assert.deepEqual(restore.missing, []);
  await restore.audit(true);
  assert.equal(restore.finalAuditComplete, false);
  assert.equal(restore.result.commandsUnforwarded, 'incomplete');
  assert.deepEqual(restore.missing, ['forwarding audit unavailable']);
});

test('published audit policy never erases positive forwarding from an earlier snapshot', async () => {
  const restore = new Restore({ forwardAudit: true, tag: 'forwarding-contract' });
  restore.result.probes = [{ id: 1, method: 'Runtime.evaluate', params: {}, sentAt: 0 }];
  restore.gate = {
    audit: async ({ final }) => ({
      complete: true,
      probes: [{ id: 1, complete: true, forwarded: !final }],
    }),
  };
  await restore.audit(false);
  assert.equal(restore.result.commandsUnforwarded, false);
  await restore.audit(true);
  assert.equal(restore.finalAuditComplete, true);
  assert.equal(restore.result.commandsUnforwarded, false);
});
