import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { launchBrowser, rawSocket } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { startServer } from './server.mjs';
import './watchdog.mjs';

const binary = path.resolve(
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);

test('foreground dialog holder retains observed Chrome dialogs and answers stdin requests after opening', async () => {
  const site = await startServer();
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-dialog-held-chrome-'));
  let browser;
  let raw;
  let child;
  let rawSession;
  try {
    browser = await launchBrowser();
    raw = await rawSocket(browser.wsUrl);
    const targetId = await raw.open(`${site.origin}/cb`);
    ({ sessionId: rawSession } = await raw.send('Target.attachToTarget', {
      targetId,
      flatten: true,
    }));
    await raw.send('Page.enable', {}, rawSession);
    child = spawn(binary, ['--cdp', browser.wsUrl, 'dialog-hold', '--tab', targetId], {
      cwd: home,
      env: { ...process.env, HOME: home, USERPROFILE: home, XDG_CONFIG_HOME: home },
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    let pending = '';
    let stderr = '';
    const events = [];
    const watchers = new Set();
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
    });
    child.stdout.on('data', (chunk) => {
      pending += chunk;
      for (;;) {
        const end = pending.indexOf('\n');
        if (end < 0) break;
        const event = JSON.parse(pending.slice(0, end));
        pending = pending.slice(end + 1);
        events.push(event);
        for (const watcher of watchers) watcher(event);
      }
    });
    const ended = new Promise((resolve, reject) => {
      child.on('error', reject);
      child.on('close', (code) => resolve({ code, stderr }));
    });
    const wait = (predicate) => {
      const found = events.find(predicate);
      if (found) return Promise.resolve(found);
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          watchers.delete(watcher);
          reject(new Error(`holder event timeout: ${stderr}`));
        }, 15000);
        const watcher = (event) => {
          if (predicate(event)) {
            clearTimeout(timer);
            watchers.delete(watcher);
            resolve(event);
          }
        };
        watchers.add(watcher);
        ended.then((result) => {
          clearTimeout(timer);
          watchers.delete(watcher);
          reject(new Error(`holder ended: ${result.stderr}`));
        }, reject);
      });
    };
    const ready = await wait((event) => event.event === 'ready');
    assert.equal(ready.target, targetId);
    assert.ok(Object.hasOwn(ready, 'loaderId'));
    child.stdin.write(`${JSON.stringify({ id: 1, dialogId: 1, command: 'dialog-dismiss' })}\n`);
    assert.equal((await wait((event) => event.id === 1)).event, 'no-dialog');
    let previous = 0;
    for (const [id, expression, command, text, answer] of [
      [2, "window.answer=prompt('name?')", 'dialog-accept', 'Lars', 'Lars'],
      [3, "window.answer=confirm('sure?')", 'dialog-dismiss', null, 'false'],
    ]) {
      const evaluated = raw.send(
        'Runtime.evaluate',
        { expression, returnByValue: true },
        rawSession
      );
      const open = await wait(
        (event) => event.event === 'dialog-opened' && event.dialogId > previous
      );
      previous = open.dialogId;
      child.stdin.write(
        `${JSON.stringify({ id, dialogId: open.dialogId, command, ...(text ? { promptText: text } : {}) })}\n`
      );
      assert.equal((await wait((event) => event.id === id)).event, 'handled');
      await evaluated;
      const value = await raw.send(
        'Runtime.evaluate',
        { expression: 'String(window.answer)', returnByValue: true },
        rawSession
      );
      assert.equal(value.result.value, answer);
    }
    child.stdin.end();
    assert.equal((await ended).code, 0, stderr);
    assert.ok(events.some((event) => event.event === 'stopped' && event.reason === 'stdin-eof'));
  } finally {
    if (raw) {
      await raw.send('Page.handleJavaScriptDialog', { accept: false }, rawSession).catch(() => {});
      raw.close();
    }
    child?.kill('SIGKILL');
    await browser?.close();
    await site.close();
    await rm(home, { recursive: true, force: true });
  }
});
