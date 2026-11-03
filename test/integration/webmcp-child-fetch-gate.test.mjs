import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { webmcpBackend } from './webmcp-backend.mjs';

await test('fake: a child queue sends no live read before a paused root Document continues and the committed root installs once', async () => {
  const cwd = await mkdtemp(path.join(tmpdir(), 'thr_uzh6iuph2q-child-fetch-'));
  const fetchSessions = new Set();
  let backend;
  let waiting = false;
  let continued = false;
  let readsBeforeContinue = 0;
  const sources = [];
  backend = await webmcpBackend({
    children: [{ id: 'CHILD', origin: 'https://ordinary.test' }],
    onCommand(command, pages, send) {
      if (command.method === 'Fetch.enable') fetchSessions.add(command.sessionId);
      if (waiting && command.method === 'Page.getFrameTree') readsBeforeContinue++;
      if (
        command.method === 'Runtime.evaluate' &&
        command.params.expression.startsWith('const __wmcp_')
      )
        sources.push({
          unique: command.params.uniqueContextId,
          continued,
          root: `main-${pages.get('TAB').loader}`,
        });
      if (command.method === 'Page.reload') {
        waiting = true;
        send({ id: command.id, result: {} });
        for (const sessionId of [command.sessionId, ...fetchSessions])
          send({
            method: 'Page.frameStartedNavigating',
            sessionId,
            params: {
              frameId: 'TAB',
              loaderId: 'RELOAD1-TAB',
              url: 'https://ordinary.test/',
              navigationType: 'reload',
            },
          });
        const sessionId = [...fetchSessions][0];
        assert.ok(sessionId);
        send({
          method: 'Fetch.requestPaused',
          sessionId,
          params: {
            requestId: 'root-document',
            frameId: 'TAB',
            resourceType: 'Document',
            request: { url: 'https://ordinary.test/' },
          },
        });
        return true;
      }
      if (
        command.method === 'Fetch.continueRequest' &&
        command.params.requestId === 'root-document'
      ) {
        waiting = false;
        continued = true;
        send({ id: command.id, result: {} });
        backend.reload();
        return true;
      }
    },
  });
  const holder = spawn(
    path.resolve('target/debug/playwright-cli'),
    ['--cdp', backend.url, 'webmcp-polyfill', '--tab', 'TAB', '--reload'],
    {
      cwd,
      env: { ...process.env, HOME: cwd, XDG_CONFIG_HOME: cwd, SLICC_CDP_URL: '' },
      stdio: ['pipe', 'pipe', 'pipe'],
    }
  );
  let stdout = '';
  let stderr = '';
  const ended = new Promise((resolve, reject) => {
    holder.once('error', reject);
    holder.once('close', (code) => resolve({ code, stdout, stderr }));
  });
  const ready = new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`holder readiness did not arrive: ${stderr}`)),
      10000
    );
    holder.stdout.on('data', (data) => {
      stdout += data;
      if (!stdout.includes('\n')) return;
      clearTimeout(timer);
      resolve(JSON.parse(stdout.split('\n')[0]));
    });
    holder.stderr.on('data', (data) => {
      stderr += data;
    });
    ended.then((result) => {
      clearTimeout(timer);
      if (!stdout.includes('\n')) reject(new Error(result.stderr));
    }, reject);
  });
  try {
    const status = await ready;
    assert.equal(status.event, 'ready');
    assert.equal(continued, true);
    assert.equal(readsBeforeContinue, 0);
    assert.equal(sources.filter(({ unique, root }) => unique === root).length, 1);
    assert.ok(sources.every(({ continued }) => continued));
    assert.equal(backend.calls.filter(({ method }) => method === 'Page.reload').length, 1);
    holder.stdin.end();
    const result = await ended;
    assert.equal(result.code, 0, result.stderr);
  } finally {
    if (holder.exitCode === null) holder.kill('SIGKILL');
    await backend.close();
    await rm(cwd, { recursive: true, force: true });
  }
});
