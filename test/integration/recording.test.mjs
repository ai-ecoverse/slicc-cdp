import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import path from 'node:path';
import { launchBrowser, rawSocket } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { nativeRunner } from './runners.mjs';
import { startServer } from './server.mjs';
import './watchdog.mjs';

const exe = (name) =>
  path.resolve('target/debug', process.platform === 'win32' ? `${name}.exe` : name);

function holder(wsUrl, args, cwd) {
  const env = { ...process.env };
  delete env.SLICC_CDP_URL;
  const child = spawn(exe('playwright-cli'), ['--cdp', wsUrl, ...args], {
    cwd,
    env,
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  let stdout = '';
  let stderr = '';
  const waiting = [];
  child.stdout.on('data', (chunk) => {
    stdout += chunk;
    for (const wake of waiting.splice(0)) wake();
  });
  child.stderr.on('data', (chunk) => {
    stderr += chunk;
  });
  const timer = setTimeout(() => child.kill('SIGKILL'), 120000);
  const ended = new Promise((resolve) => {
    child.on('close', (code) => {
      clearTimeout(timer);
      for (const wake of waiting.splice(0)) wake();
      resolve({ code, stdout, stderr });
    });
  });
  const events = () =>
    stdout
      .split('\n')
      .filter(Boolean)
      .map((line) => JSON.parse(line));
  const event = async (name) => {
    for (;;) {
      const found = events().find((item) => item.event === name);
      if (found) return found;
      if (child.exitCode !== null) throw new Error(`holder exited before ${name}: ${stderr}`);
      await new Promise((resolve) => waiting.push(resolve));
    }
  };
  return { child, ended, event, events };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 700));

test('recording-start records trusted Chrome actions and nothing from protected documents', async (t) => {
  const site = await startServer();
  let browser;
  let raw;
  const holders = [];
  try {
    browser = await launchBrowser([
      '--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1, MAP vault.sliccy.ai 127.0.0.1',
      '--no-proxy-server',
    ]);
    raw = await rawSocket(browser.wsUrl);
    const target = await raw.open(`${site.origin}/page`);
    const { sessionId } = await raw.send('Target.attachToTarget', {
      targetId: target,
      flatten: true,
    });
    await raw.send('Page.enable', {}, sessionId);
    const runner = nativeRunner(exe('playwright-cli'), exe('curlwright'), browser.wsUrl);
    let cwd = await runner.fresh();
    const start = async () => {
      for (const held of holders) held.child.kill('SIGKILL');
      cwd = await runner.fresh();
      const held = holder(browser.wsUrl, ['recording-start', '--tab', target], cwd);
      holders.push(held);
      return held;
    };
    const cli = async (args) => {
      const result = await runner.playwright([...args, '--tab', target], cwd);
      assert.equal(result.code, 0, `${args.join(' ')}: ${result.stderr}`);
      return result.stdout;
    };
    const patterns = {
      go: /button "Go".*\[ref=(e\d+)\]/,
      name: /textbox "Name".*\[ref=(e\d+)\]/,
      agree: /checkbox "Agree".*\[ref=(e\d+)\]/,
    };
    const act = async (command, element, ...rest) => {
      const snapshot = await cli(['snapshot']);
      const ref = snapshot.match(patterns[element])?.[1];
      assert.ok(ref, snapshot);
      return cli([command, ref, ...rest]);
    };

    await t.test('trusted clicks, fills, presses and checks become Playwright code', async () => {
      const held = await start();
      const ready = await held.event('ready');
      assert.equal(ready.tab, target);
      await act('click', 'go');
      await act('fill', 'name', 'hello');
      await cli(['press', 'Enter']);
      await act('check', 'agree');
      await raw.send(
        'Runtime.evaluate',
        { expression: "document.getElementById('go').click()" },
        sessionId
      );
      await settle();
      const stopped = await runner.playwright(['recording-stop'], cwd);
      assert.equal(stopped.code, 0, stopped.stderr);
      assert.ok(
        stopped.stdout.startsWith('### Result\nRecording stopped. Recorded actions:\n\n```js\n'),
        stopped.stdout
      );
      for (const line of [
        "await page.getByRole('button', { name: 'Go' }).click();",
        "await page.getByRole('textbox', { name: 'Name' }).fill('hello');",
        "await page.getByRole('textbox', { name: 'Name' }).press('Enter');",
        "await page.getByRole('checkbox', { name: 'Agree' }).check();",
      ])
        assert.ok(stopped.stdout.includes(`${line}\n`), `${line}\n${stopped.stdout}`);
      assert.equal(stopped.stdout.match(/name: 'Go' \}\)\.click\(\)/g).length, 1, stopped.stdout);
      const ended = await held.ended;
      assert.equal(ended.code, 0, ended.stderr);
      assert.equal(held.events().at(-1).reason, 'stop');
    });

    await t.test('input between two recordings on one document is never recorded', async () => {
      const first = await start();
      await first.event('ready');
      await settle();
      const empty = await runner.playwright(['recording-stop'], cwd);
      assert.equal(empty.code, 0, empty.stderr);
      assert.equal((await first.ended).code, 0);
      await act('fill', 'name', 'private-between-recordings');
      await settle();
      const second = await start();
      await second.event('ready');
      await settle();
      const stopped = await runner.playwright(['recording-stop'], cwd);
      assert.equal(stopped.code, 0, stopped.stderr);
      assert.equal(stopped.stdout, '### Result\nRecording stopped. No actions were recorded.\n');
      const ended = await second.ended;
      assert.ok(!ended.stdout.includes('private-between-recordings'), ended.stdout);
    });

    await t.test(
      'a protected navigation never records anything from the protected document',
      async () => {
        const held = await start();
        await held.event('ready');
        await act('click', 'go');
        await settle();
        await raw
          .send(
            'Page.navigate',
            { url: `http://seven.sliccy.ai:${site.port}/page?secret=hidden` },
            sessionId
          )
          .catch(() => {});
        await settle();
        for (const type of ['mousePressed', 'mouseReleased'])
          await raw.send(
            'Input.dispatchMouseEvent',
            { type, x: 10, y: 10, button: 'left', clickCount: 1 },
            sessionId
          );
        await raw.send('Input.insertText', { text: 'seven-secret' }, sessionId);
        await settle();
        const early = held.events().find((item) => item.event === 'stopped');
        let printed;
        let ended;
        if (early) {
          assert.equal(early.reason, 'refused');
          ended = await held.ended;
          assert.equal(ended.code, 1);
          assert.match(ended.stderr, /refused/);
          printed = await runner.playwright(['recording-stop'], cwd);
        } else {
          assert.ok(
            held.events().some((item) => item.event === 'paused'),
            JSON.stringify(held.events())
          );
          await raw.send('Page.navigate', { url: `${site.origin}/page` }, sessionId);
          await held.event('resumed');
          printed = await runner.playwright(['recording-stop'], cwd);
          ended = await held.ended;
          assert.equal(ended.code, 0, ended.stderr);
        }
        assert.equal(printed.code, 0, printed.stderr);
        assert.ok(
          printed.stdout.includes("await page.getByRole('button', { name: 'Go' }).click();\n"),
          printed.stdout
        );
        for (const text of [printed.stdout, ended.stdout, ended.stderr]) {
          assert.ok(!text.includes('hidden'), text);
          assert.ok(!text.includes('seven-secret'), text);
          assert.ok(!text.includes('seven.sliccy.ai'), text);
        }
      }
    );
  } finally {
    for (const held of holders) held.child.kill('SIGKILL');
    raw?.close();
    await browser?.close();
    await site.close();
  }
});
