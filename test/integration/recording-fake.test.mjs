import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readdir, realpath, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { HARNESS_PORTS } from './ports.mjs';
import { kernelRunner, kernelSessionFile } from './runners.mjs';
import './watchdog.mjs';

const binary =
  process.env.SLICC_RECORDING_BIN ||
  path.resolve(
    'target/debug',
    process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
  );
const NONE = 'Error: No recording in progress, use browser_start_recording to start one.\n';

function environment(home) {
  const env = {
    ...process.env,
    HOME: home,
    USERPROFILE: home,
    APPDATA: home,
    XDG_CONFIG_HOME: home,
    XDG_STATE_HOME: home,
    SLICC_CDP_PORTS: HARNESS_PORTS,
  };
  delete env.SLICC_CDP_URL;
  return env;
}

function launch(args, home) {
  const child = spawn(binary, args, {
    cwd: home,
    env: environment(home),
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  let stdout = '';
  let stderr = '';
  const waiting = [];
  const lines = () =>
    stdout
      .split('\n')
      .filter(Boolean)
      .map((line) => JSON.parse(line));
  child.stdout.on('data', (chunk) => {
    stdout += chunk;
    for (const waiter of waiting.splice(0)) waiter();
  });
  child.stderr.on('data', (chunk) => {
    stderr += chunk;
  });
  const timer = setTimeout(() => child.kill('SIGKILL'), 30000);
  const ended = new Promise((resolve) => {
    child.on('close', (code) => {
      clearTimeout(timer);
      for (const waiter of waiting.splice(0)) waiter();
      resolve({ code, stdout, stderr });
    });
  });
  const event = async (name) => {
    for (;;) {
      const found = lines().find((line) => line.event === name);
      if (found) return found;
      if (child.exitCode !== null) throw new Error(`holder exited before ${name}: ${stderr}`);
      await new Promise((resolve) => waiting.push(resolve));
    }
  };
  return { child, ended, event, lines, stderr: () => stderr };
}

async function run(args, home) {
  const process = launch(args, home);
  process.child.stdin.end();
  return process.ended;
}

const role = (name) => ({ kind: 'role', role: 'button', name });
const ACTIONS = [
  { name: 'click', locator: role('Go'), button: 'left', modifiers: 0, clickCount: 1 },
  { name: 'click', locator: role('Go'), button: 'left', modifiers: 0, clickCount: 2 },
  { name: 'fill', locator: { kind: 'label', value: 'Email' }, text: 'a' },
  { name: 'fill', locator: { kind: 'label', value: 'Email' }, text: "a'b\n" },
  { name: 'press', locator: { kind: 'label', value: 'Email' }, key: 'Enter', modifiers: 10 },
  { name: 'check', locator: { kind: 'role', role: 'checkbox', name: 'Agree', exact: true } },
  { name: 'uncheck', locator: { kind: 'testid', value: 'opt' } },
  { name: 'select', locator: { kind: 'placeholder', value: 'Pick' }, options: ['a', 'b'] },
  { name: 'setInputFiles', locator: { kind: 'css', value: '#file' }, files: ['a.txt'] },
  {
    name: 'click',
    locator: { kind: 'text', value: 'More', nth: 0 },
    button: 'right',
    modifiers: 1,
    clickCount: 1,
    position: { x: 3, y: 4.5 },
  },
  {
    name: 'click',
    locator: { kind: 'role', role: 'link', name: 'Next', exact: true, nth: 2 },
    button: 'middle',
    modifiers: 0,
    clickCount: 3,
  },
  { name: 'press', locator: { kind: 'css', value: 'body' }, key: 'Escape', modifiers: 0 },
  { name: 'unknown', locator: role('Ignored') },
  { name: 'click', locator: { kind: 'role', role: 'Bad Role' }, clickCount: 1 },
  { name: 'click', locator: role('Bad'), button: 'back', clickCount: 1 },
  { name: 'fill', locator: { kind: 'nope', value: 'x' }, text: 'x' },
];
const CODE = [
  "await page.getByRole('button', { name: 'Go' }).dblclick();",
  "await page.getByLabel('Email').fill('a\\'b\\n');",
  "await page.getByLabel('Email').press('ControlOrMeta+Shift+Enter');",
  "await page.getByRole('checkbox', { name: 'Agree', exact: true }).check();",
  "await page.getByTestId('opt').uncheck();",
  "await page.getByPlaceholder('Pick').selectOption(['a', 'b']);",
  "await page.locator('#file').setInputFiles('a.txt');",
  "await page.getByText('More').first().click({\n  button: 'right',\n  modifiers: ['Alt'],\n  position: {\n    x: 3,\n    y: 4.5\n  }\n});",
  "await page.getByRole('link', { name: 'Next', exact: true }).nth(2).click({\n  button: 'middle',\n  clickCount: 3\n});",
  "await page.locator('body').press('Escape');",
];
const STOPPED = `### Result\nRecording stopped. Recorded actions:\n\n\`\`\`js\n${CODE.join('\n')}\n\`\`\`\n`;

function recorder(options = {}) {
  const queue = [...(options.actions ?? [])];
  const installs = [];
  const intercept = (message, send) => {
    const expression = message.params?.expression ?? '';
    if (message.method !== 'Runtime.evaluate') return options.intercept?.(message, send) ?? false;
    const answer = (value) =>
      send({ id: message.id, sessionId: message.sessionId, result: { result: value } });
    if (expression.includes('globalThis.__sliccRecording = (() =>')) {
      installs.push(message);
      answer({ type: 'string', value: options.href?.() ?? 'https://example.test/' });
      return true;
    }
    if (expression.startsWith('globalThis.__sliccRecording ?')) {
      const items = queue.splice(0);
      setTimeout(
        () => answer({ type: 'object', subtype: 'array', value: items }),
        items.length ? 0 : 40
      );
      return true;
    }
    return options.intercept?.(message, send) ?? false;
  };
  return { queue, installs, intercept };
}

async function withFake(options, body) {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-recording-'));
  const fake = await fakeCdp(options);
  fake.targets.push({ targetId: 'TAB', url: 'https://example.test/' });
  try {
    await body(fake, home, (args) => [`--cdp`, fake.url, ...args]);
  } finally {
    await fake.close();
    await rm(home, { recursive: true, force: true });
  }
}

async function recordingFile(home) {
  const key = createHash('sha256')
    .update(await realpath(home))
    .digest('hex');
  const base =
    process.platform === 'darwin' ? path.join(home, 'Library', 'Application Support') : home;
  return path.join(base, 'slicc', 'playwright-cli', 'sessions', `${key}.recording.json`);
}

const KEPT = {
  owner: 'gone',
  tab: 'TAB',
  state: 'stopped',
  reason: 'closed',
  actions: ["await page.getByRole('button', { name: 'Go' }).click();"],
};
const KEPT_TEXT =
  "### Result\nRecording stopped. Recorded actions:\n\n```js\nawait page.getByRole('button', { name: 'Go' }).click();\n```\n";

async function recordingFiles(home) {
  const names = [];
  const walk = async (dir) => {
    for (const entry of await readdir(dir, { withFileTypes: true }).catch(() => [])) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) await walk(full);
      else if (entry.name.includes('.recording.')) names.push(full);
    }
  };
  await walk(home);
  return names;
}

test('help pages and global help list the recording commands like Microsoft', async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-recording-help-'));
  try {
    const global = await run(['--help'], home);
    assert.equal(global.code, 0);
    for (const line of [
      '  recording-start             start recording user actions',
      '  recording-stop              stop recording user actions and print them as playwright code',
      '  record                      alias of recording-start',
      '  stop-recording              alias of recording-stop',
    ])
      assert.ok(global.stdout.includes(`${line}\n`), line);
    const pending = global.stdout.split('Not implemented yet:')[1].split('\n')[1];
    for (const name of ['record', 'stop-recording', 'recording-start', 'recording-stop'])
      assert.ok(!pending.split(', ').includes(name), name);
    for (const name of ['recording-start', 'record']) {
      const page = await run([name, '--help'], home);
      assert.equal(page.code, 0);
      assert.ok(
        page.stdout.startsWith(`playwright-cli ${name} \n\nStart recording user actions\n`)
      );
      assert.match(page.stdout, /--tab <targetId> +tab to record/);
      assert.ok(!page.stdout.includes('Not implemented'));
    }
    for (const name of ['recording-stop', 'stop-recording']) {
      const page = await run([name, '--help'], home);
      assert.equal(page.code, 0);
      assert.ok(
        page.stdout.startsWith(
          `playwright-cli ${name} \n\nStop recording user actions and print them as Playwright code\n`
        )
      );
    }
    const extra = await run(['record', 'https://example.test/'], home);
    assert.equal(extra.code, 1);
    assert.equal(extra.stderr, 'error: too many arguments: expected 0, received 1\n');
    const flag = await run(['recording-stop', '--tab', 'TAB'], home);
    assert.equal(flag.code, 1);
    assert.match(flag.stderr, /Unknown option/);
  } finally {
    await rm(home, { recursive: true, force: true });
  }
});

test('recording-stop without a holder fails like Microsoft and never dials the browser', async () => {
  await withFake({}, async (fake, home, cli) => {
    for (const name of ['recording-stop', 'stop-recording']) {
      const result = await run(cli([name]), home);
      assert.equal(result.code, 1);
      assert.equal(result.stdout, '');
      assert.equal(result.stderr, NONE);
    }
    assert.equal(fake.calls.length, 0);
  });
});

test('the holder records verified ordinary actions and recording-stop prints Playwright code', async () => {
  const fake = recorder({ actions: ACTIONS });
  await withFake({ intercept: fake.intercept }, async (cdp, home, cli) => {
    const holder = launch(cli(['recording-start', '--tab', 'TAB']), home);
    const ready = await holder.event('ready');
    assert.equal(ready.tab, 'TAB');
    const again = await run(cli(['record', '--tab', 'TAB']), home);
    assert.equal(again.code, 1);
    assert.equal(again.stderr, 'Error: Recording is already in progress.\n');
    await new Promise((resolve) => setTimeout(resolve, 300));
    const stopped = await run(cli(['recording-stop']), home);
    assert.equal(stopped.code, 0, stopped.stderr);
    assert.equal(stopped.stdout, STOPPED);
    const ended = await holder.ended;
    assert.equal(ended.code, 0, ended.stderr);
    const last = holder.lines().at(-1);
    assert.equal(last.event, 'stopped');
    assert.equal(last.reason, 'stop');
    assert.deepEqual(last.actions, CODE);
    assert.deepEqual(await recordingFiles(home), []);
    assert.ok(cdp.calls.some((call) => call.method === 'Target.activateTarget'));
    const disposed = cdp.calls.filter(
      (call) =>
        call.method === 'Runtime.evaluate' &&
        call.params.expression.includes('__sliccRecording?.dispose?.()') &&
        !call.params.expression.includes('sliccRecorder(')
    );
    assert.equal(disposed.length, 1);
    assert.ok(
      fake.installs.every((call) =>
        call.params.expression.startsWith('(() => { globalThis.__sliccRecording?.dispose?.();')
      )
    );
    assert.ok(
      cdp.calls.some(
        (call) =>
          call.method === 'Page.createIsolatedWorld' && call.params.worldName === 'slicc-recorder'
      )
    );
    for (const method of [
      'Runtime.addBinding',
      'Page.addScriptToEvaluateOnNewDocument',
      'Page.startScreencast',
      'Tracing.start',
    ])
      assert.ok(
        cdp.calls.every((call) => call.method !== method),
        method
      );
    const after = await run(cli(['recording-stop']), home);
    assert.equal(after.stderr, NONE);
  });
});

test('closing stdin ends the holder with its actions and an empty recording prints nothing', async () => {
  const fake = recorder({ actions: ACTIONS.slice(0, 1) });
  await withFake({ intercept: fake.intercept }, async (cdp, home, cli) => {
    const holder = launch(cli(['record', '--tab', 'TAB']), home);
    await holder.event('ready');
    await new Promise((resolve) => setTimeout(resolve, 200));
    holder.child.stdin.end();
    const ended = await holder.ended;
    assert.equal(ended.code, 0, ended.stderr);
    assert.deepEqual(holder.lines().at(-1), {
      event: 'stopped',
      tab: 'TAB',
      reason: 'stdin',
      actions: ["await page.getByRole('button', { name: 'Go' }).click();"],
    });
    assert.deepEqual(await recordingFiles(home), []);
    const none = await run(cli(['stop-recording']), home);
    assert.equal(none.stderr, NONE);
  });
  const empty = recorder();
  await withFake({ intercept: empty.intercept }, async (cdp, home, cli) => {
    const holder = launch(cli(['recording-start']), home);
    await holder.event('ready');
    const stopped = await run(cli(['stop-recording']), home);
    assert.equal(stopped.code, 0, stopped.stderr);
    assert.equal(stopped.stdout, '### Result\nRecording stopped. No actions were recorded.\n');
    assert.equal((await holder.ended).code, 0);
  });
});

test('a tab that turns protected stops capture, keeps earlier actions and never injects there', async () => {
  let session;
  let send;
  const fake = recorder({
    actions: ACTIONS.slice(0, 1),
    intercept: (message, reply) => {
      if (message.method === 'Page.createIsolatedWorld') {
        session = message.sessionId;
        send = reply;
      }
      return false;
    },
  });
  await withFake({ intercept: fake.intercept }, async (cdp, home, cli) => {
    const holder = launch(cli(['recording-start', '--tab', 'TAB']), home);
    await holder.event('ready');
    await new Promise((resolve) => setTimeout(resolve, 200));
    const installs = fake.installs.length;
    const target = cdp.targets.find((item) => item.targetId === 'TAB');
    target.url = 'https://seven.sliccy.ai/?secret=hidden';
    target.loaderId = 'SEVEN';
    fake.queue.push({ name: 'fill', locator: { kind: 'label', value: 'Secret' }, text: 'hidden' });
    send({
      method: 'Page.frameNavigated',
      sessionId: session,
      params: {
        frame: {
          id: 'TAB',
          loaderId: 'SEVEN',
          url: target.url,
          securityOrigin: 'https://seven.sliccy.ai',
        },
      },
    });
    const ended = await holder.ended;
    assert.equal(ended.code, 1);
    assert.match(ended.stderr, /refused/);
    const stopped = holder.lines().at(-1);
    assert.equal(stopped.reason, 'refused');
    assert.deepEqual(stopped.actions, ["await page.getByRole('button', { name: 'Go' }).click();"]);
    assert.ok(!ended.stdout.includes('hidden'));
    assert.ok(!ended.stderr.includes('hidden'));
    assert.equal(fake.installs.length, installs);
    const printed = await run(cli(['recording-stop']), home);
    assert.equal(printed.code, 0, printed.stderr);
    assert.ok(printed.stdout.includes("getByRole('button', { name: 'Go' }).click()"));
    assert.ok(!printed.stdout.includes('hidden'));
    assert.deepEqual(await recordingFiles(home), []);
  });
});

test('an unverified document pauses capture and the next ordinary document records a goto', async () => {
  let session;
  let send;
  let href = 'https://example.test/';
  const fake = recorder({
    actions: [ACTIONS[5]],
    href: () => href,
    intercept: (message, reply) => {
      if (message.method === 'Page.createIsolatedWorld') {
        session = message.sessionId;
        send = reply;
      }
      return false;
    },
  });
  await withFake({ intercept: fake.intercept }, async (cdp, home, cli) => {
    const holder = launch(cli(['recording-start', '--tab', 'TAB']), home);
    await holder.event('ready');
    await new Promise((resolve) => setTimeout(resolve, 200));
    const target = cdp.targets.find((item) => item.targetId === 'TAB');
    const navigate = (url, loaderId) => {
      target.url = url;
      target.loaderId = loaderId;
      href = url;
      send({
        method: 'Page.frameNavigated',
        sessionId: session,
        params: { frame: { id: 'TAB', loaderId, url, securityOrigin: new URL(url).origin } },
      });
    };
    navigate('about:blank', 'BLANK');
    await holder.event('paused');
    const installs = fake.installs.length;
    await new Promise((resolve) => setTimeout(resolve, 400));
    assert.equal(fake.installs.length, installs);
    navigate('https://other.test/next?token=hidden', 'NEXT');
    await holder.event('resumed');
    holder.child.stdin.end();
    const ended = await holder.ended;
    assert.equal(ended.code, 0, ended.stderr);
    assert.deepEqual(holder.lines().at(-1).actions, [
      "await page.getByRole('checkbox', { name: 'Agree', exact: true }).check();",
      "await page.goto('https://other.test/next?token=…');",
    ]);
    assert.ok(!ended.stdout.includes('hidden'));
  });
});

test('a closed tab ends the holder and recording-stop prints the kept actions once', async () => {
  let send;
  const fake = recorder({
    actions: ACTIONS.slice(0, 1),
    intercept: (message, reply) => {
      if (message.method === 'Page.createIsolatedWorld') send = reply;
      return false;
    },
  });
  await withFake({ intercept: fake.intercept }, async (cdp, home, cli) => {
    const holder = launch(cli(['recording-start', '--tab', 'TAB']), home);
    await holder.event('ready');
    await new Promise((resolve) => setTimeout(resolve, 200));
    send({ method: 'Target.targetDestroyed', params: { targetId: 'TAB' } });
    const ended = await holder.ended;
    assert.equal(ended.code, 0, ended.stderr);
    assert.equal(holder.lines().at(-1).reason, 'closed');
    const printed = await run(cli(['recording-stop']), home);
    assert.equal(printed.code, 0, printed.stderr);
    assert.equal(printed.stdout, KEPT_TEXT);
    assert.equal((await run(cli(['recording-stop']), home)).stderr, NONE);
  });
});

test('recording-stop prints a kept result and drops a stale holder without dialing', async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-recording-kept-'));
  try {
    const file = await recordingFile(home);
    await mkdir(path.dirname(file), { recursive: true });
    await writeFile(file, JSON.stringify(KEPT));
    const printed = await run(
      ['--cdp', 'ws://127.0.0.1:1/devtools/browser/x', 'stop-recording'],
      home
    );
    assert.equal(printed.code, 0, printed.stderr);
    assert.equal(printed.stdout, KEPT_TEXT);
    await writeFile(file, JSON.stringify({ owner: 'dead', state: 'recording', beat: 1 }));
    const dropped = await run(['recording-stop'], home);
    assert.equal(dropped.code, 1);
    assert.equal(dropped.stderr, NONE);
    await assert.rejects(stat(file));
  } finally {
    await rm(home, { recursive: true, force: true });
  }
});

test('protected and unverifiable tabs are refused before any recorder is installed', async () => {
  const fake = recorder();
  await withFake({ intercept: fake.intercept }, async (cdp, home, cli) => {
    for (const url of ['https://seven.sliccy.ai/?secret=hidden', 'http://vault.sliccy.ai/', '']) {
      cdp.targets.push({ targetId: 'REFUSED', url });
      const result = await run(cli(['recording-start', '--tab', 'REFUSED']), home);
      assert.equal(result.code, 1, url);
      assert.equal(result.stdout, '');
      assert.ok(!result.stderr.includes('hidden'));
      cdp.targets.pop();
    }
    assert.equal(fake.installs.length, 0);
    assert.ok(
      cdp.calls.every(
        (call) => call.method !== 'Page.createIsolatedWorld' || call.target !== 'REFUSED'
      )
    );
    assert.deepEqual(await recordingFiles(home), []);
  });
});

test('kernel holder records the same actions and stops at EOF', async () => {
  const fake = recorder({ actions: ACTIONS });
  const cdp = await fakeCdp({ intercept: fake.intercept });
  cdp.targets.push({ targetId: 'TAB', url: 'https://example.test/' });
  let runner;
  try {
    runner = await kernelRunner(
      path.resolve('target/wasm32-wasip1/debug/playwright-cli.wasm'),
      path.resolve('target/wasm32-wasip1/debug/curlwright.wasm'),
      cdp.url
    );
    const cwd = await runner.fresh();
    const held = await runner.playwright(['recording-start', '--tab', 'TAB'], cwd, { stdin: '' });
    assert.equal(held.code, 0, held.stderr);
    const events = held.stdout
      .trim()
      .split('\n')
      .map((line) => JSON.parse(line));
    assert.equal(events[0].event, 'ready');
    assert.deepEqual(events.at(-1), {
      event: 'stopped',
      tab: 'TAB',
      reason: 'stdin',
      actions: CODE,
    });
    const none = await runner.playwright(['recording-stop'], cwd);
    assert.equal(none.code, 1);
    assert.equal(none.stderr, NONE);
    const kept = kernelSessionFile('/root', cwd).replace(/\.json$/, '.recording.json');
    await runner.writeFile(kept, JSON.stringify(KEPT));
    const printed = await runner.playwright(['stop-recording'], cwd);
    assert.equal(printed.code, 0, printed.stderr);
    assert.equal(printed.stdout, KEPT_TEXT);
    await runner.writeFile(kept, JSON.stringify({ owner: 'dead', state: 'recording', beat: 1 }));
    const stale = await runner.playwright(['recording-stop'], cwd);
    assert.equal(stale.stderr, NONE);
    await assert.rejects(runner.readFile(kept));
  } finally {
    await Promise.resolve(runner?.close()).catch(() => {});
    await cdp.close();
  }
});
