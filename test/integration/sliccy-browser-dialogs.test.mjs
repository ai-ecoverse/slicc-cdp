import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { after, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import browser from '../../packages/sliccy-browser/src/browser.js';
import { dialogFake, FORM, SECRET, until } from './sliccy-dialog-fake.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const scratch = await mkdtemp(path.join(tmpdir(), 'sliccy-dialogs-'));
after(() => rm(scratch, { recursive: true, force: true }));

const SEVEN = 'https://lars.sliccy.ai/';

async function explicit(fake, work) {
  const previous = process.env.SLICC_CDP_URL;
  process.env.SLICC_CDP_URL = fake.url;
  try {
    return await work();
  } finally {
    await idle();
    if (previous === undefined) delete process.env.SLICC_CDP_URL;
    else process.env.SLICC_CDP_URL = previous;
  }
}

async function warnings(work) {
  const lines = [];
  const warn = console.warn;
  console.warn = (...args) => lines.push(args.join(' '));
  try {
    await work(lines);
  } finally {
    console.warn = warn;
  }
  return lines;
}

const idle = () => new Promise((resolve) => setTimeout(resolve, 50));

async function settleRefusal(world, tab, lines, before = 0) {
  await until(
    () => browser.dialogs(tab).filter((record) => record.refused).length > before,
    'the refusal'
  );
  assert.deepEqual(world.handled(), []);
  await assert.rejects(browser.eval(tab, '1'), /may not answer/);
  const text = `${lines.join('\n')}\n${JSON.stringify(browser.dialogs(tab))}`;
  assert.ok(!text.includes(SECRET), text);
  assert.ok(!text.includes('sliccy.ai'), text);
  for (const record of browser.dialogs(tab).filter((item) => item.refused)) {
    assert.deepEqual(Object.keys(record).sort(), ['at', 'refused', 'targetId']);
  }
}

test('a dialog that opens between two evals is accepted, reported, and the second eval works', async () => {
  const world = await dialogFake();
  try {
    const lines = await warnings(() =>
      explicit(world.fake, async () => {
        const tab = await browser.ensureTab(FORM);
        assert.equal(await browser.eval(tab, '1'), 'fake-eval');
        await idle();
        world.openDialog(tab.targetId, { message: 'Actual End Date cannot be in Future' });
        await until(() => world.handled().length === 1, 'the answer');
        assert.deepEqual(world.handled()[0].params, { accept: true });
        assert.equal(world.disconnects.length, 1, 'only the idle eval connection closed');
        assert.equal(await browser.eval(tab, '2'), 'fake-eval');
        const [record] = browser.dialogs(tab);
        assert.equal(record.type, 'alert');
        assert.equal(record.message, 'Actual End Date cannot be in Future');
        assert.equal(record.accepted, true);
        assert.equal(record.by, 'default');
        assert.equal(record.url, FORM);
      })
    );
    assert.equal(lines.length, 1, lines.join('\n'));
    assert.match(lines[0], /alert dialog nobody answered, so it was accepted/);
    assert.match(lines[0], /"Actual End Date cannot be in Future"/);
  } finally {
    await world.fake.close();
  }
});

test('an eval that opens a dialog itself returns once the holder answers it', async () => {
  const world = await dialogFake({
    hold(message) {
      if (message.method === 'Runtime.evaluate' && message.params?.expression === 'confirm("go?")')
        world.openDialog(
          world.fake.calls.find((call) => call.sessionId === message.sessionId)?.target,
          { type: 'confirm', message: 'go?' }
        );
      return false;
    },
  });
  try {
    await warnings(() =>
      explicit(world.fake, async () => {
        const tab = await browser.ensureTab(FORM);
        await browser.eval(tab, '1');
        assert.equal(await browser.eval(tab, 'confirm("go?")'), 'after-dialog');
        assert.deepEqual(world.handled()[0].params, { accept: false });
      })
    );
  } finally {
    await world.fake.close();
  }
});

test('confirm and prompt nobody answers are dismissed, and beforeunload is accepted', async () => {
  const world = await dialogFake();
  try {
    const lines = await warnings(() =>
      explicit(world.fake, async () => {
        const tab = await browser.ensureTab(FORM);
        await browser.eval(tab, '1');
        for (const [index, type] of ['confirm', 'prompt', 'beforeunload'].entries()) {
          world.openDialog(tab.targetId, { type, message: `m-${type}`, defaultPrompt: 'd' });
          await until(() => world.handled().length === index + 1, type);
        }
        assert.deepEqual(
          world.handled().map((call) => call.params),
          [{ accept: false }, { accept: false }, { accept: true }]
        );
        await browser.eval(tab, '2');
        assert.deepEqual(
          browser.dialogs(tab).map(({ type, accepted, by }) => [type, accepted, by]),
          [
            ['confirm', false, 'default'],
            ['prompt', false, 'default'],
            ['beforeunload', true, 'default'],
          ]
        );
      })
    );
    assert.equal(lines.length, 3);
    assert.match(lines[1], /a prompt dialog nobody answered, so it was dismissed: "m-prompt"/);
    assert.match(lines[2], /a beforeunload dialog nobody answered, so it was accepted/);
  } finally {
    await world.fake.close();
  }
});

test('onDialog answers with the handler, and falls back when the handler does not answer', async () => {
  const world = await dialogFake();
  try {
    const lines = await warnings(() =>
      explicit(world.fake, async () => {
        const tab = await browser.ensureTab(FORM);
        const seen = [];
        const handle = await browser.onDialog(tab, async (dialog) => {
          seen.push({ ...dialog });
          if (dialog.type === 'prompt') assert.equal(await dialog.accept('Lars'), true);
        });
        world.openDialog(tab.targetId, { type: 'prompt', message: 'name?', defaultPrompt: 'x' });
        await until(() => world.handled().length === 1, 'the handler answer');
        assert.deepEqual(world.handled()[0].params, { accept: true, promptText: 'Lars' });
        world.openDialog(tab.targetId, { type: 'confirm', message: 'sure?' });
        await until(() => world.handled().length === 2, 'the fallback');
        assert.deepEqual(world.handled()[1].params, { accept: false });
        assert.deepEqual(
          seen.map(({ targetId, type, message, url }) => ({ targetId, type, message, url })),
          [
            { targetId: tab.targetId, type: 'prompt', message: 'name?', url: FORM },
            { targetId: tab.targetId, type: 'confirm', message: 'sure?', url: FORM },
          ]
        );
        assert.equal(await handle.close(), true);
        world.openDialog(tab.targetId, { type: 'alert', message: 'plain' });
        await until(() => world.handled().length === 3, 'the default');
        assert.equal(seen.length, 2);
        await browser.eval(tab, '1');
        assert.deepEqual(
          browser.dialogs(tab).map(({ type, by, promptText }) => [type, by, promptText]),
          [
            ['prompt', 'handler', 'Lars'],
            ['confirm', 'default', undefined],
            ['alert', 'default', undefined],
          ]
        );
      })
    );
    assert.equal(lines.length, 2, lines.join('\n'));
  } finally {
    await world.fake.close();
  }
});

test('a dialog of a protected owner is never answered, handed to a handler or echoed', async () => {
  const world = await dialogFake();
  try {
    let called = false;
    const lines = await warnings((lines) =>
      explicit(world.fake, async () => {
        const tab = await browser.ensureTab(SEVEN);
        await browser.onDialog(tab, () => {
          called = true;
        });
        world.openDialog(tab.targetId, { url: SEVEN });
        await settleRefusal(world, tab, lines);
      })
    );
    assert.equal(called, false);
    assert.match(lines.join('\n'), /isn't a verified ordinary page/);
  } finally {
    await world.fake.close();
  }
});

test('a dialog of a subframe or of an unknown frame is refused', async () => {
  const world = await dialogFake();
  try {
    await warnings((lines) =>
      explicit(world.fake, async () => {
        const tab = await browser.ensureTab(FORM);
        await browser.eval(tab, '1');
        world.openDialog(tab.targetId, { frameId: 'CHILD-FRAME' });
        await settleRefusal(world, tab, lines);
        world.emit(world.holdSession(tab.targetId), 'Page.javascriptDialogClosed', {});
        world.open.delete(tab.targetId);
        world.openDialog(tab.targetId, { frameId: undefined });
        await settleRefusal(world, tab, lines, 1);
      })
    );
  } finally {
    await world.fake.close();
  }
});

test('a dialog of a protected child frame is refused and blanked, and the hold stays', async () => {
  const world = await dialogFake();
  try {
    await warnings((lines) =>
      explicit(world.fake, async () => {
        const tab = await browser.ensureTab(FORM);
        await browser.eval(tab, '1');
        const [session] = world.holdSessions(tab.targetId);
        world.openDialog(tab.targetId, { frameId: 'SEVEN-CHILD', url: SEVEN });
        await settleRefusal(world, tab, lines);
        assert.deepEqual(world.holdSessions(tab.targetId), [session]);
        assert.ok(!world.detached().includes(session));
      })
    );
  } finally {
    await world.fake.close();
  }
});

test('a dialog whose owner is a wrapped protected URL, a protected error page or a protected popup is refused', async () => {
  const world = await dialogFake();
  try {
    await warnings((lines) =>
      explicit(world.fake, async () => {
        const tab = await browser.ensureTab(FORM);
        await browser.eval(tab, '1');
        const owners = [`view-source:${SEVEN}`, `blob:${SEVEN.slice(0, -1)}/7a1c`];
        for (const [index, url] of owners.entries()) {
          world.openDialog(tab.targetId, { url });
          await settleRefusal(world, tab, lines, index);
          world.emit(world.holdSession(tab.targetId), 'Page.javascriptDialogClosed', {});
          world.open.delete(tab.targetId);
        }
        const target = world.fake.targets.find((item) => item.targetId === tab.targetId);
        target.url = 'chrome-error://chromewebdata/';
        target.history = {
          currentIndex: 0,
          entries: [{ id: 1, url: SEVEN, title: '', transitionType: 'typed' }],
        };
        world.openDialog(tab.targetId, { url: SEVEN });
        await settleRefusal(world, tab, lines, 2);
        world.openDialog(tab.targetId, { url: 'chrome-error://chromewebdata/' });
        await idle();
        world.open.delete(tab.targetId);
        world.fake.targets.push({ targetId: 'POPUP', openerId: 'SEVEN', url: 'about:blank' });
        const popup = { targetId: 'POPUP' };
        await browser.eval(popup, '1');
        world.openDialog('POPUP', { url: 'about:blank' });
        await settleRefusal(world, popup, lines);
        assert.deepEqual(world.handled(), []);
      })
    );
  } finally {
    await world.fake.close();
  }
});

test('an owner that turns protected is refused and its hold is detached, by its target or by its history', async () => {
  const world = await dialogFake();
  try {
    await warnings((lines) =>
      explicit(world.fake, async () => {
        const tab = await browser.ensureTab(FORM);
        await browser.eval(tab, '1');
        const target = world.fake.targets.find((item) => item.targetId === tab.targetId);
        target.url = SEVEN;
        target.history = {
          currentIndex: 0,
          entries: [{ id: 1, url: FORM, title: '', transitionType: 'typed' }],
        };
        const first = world.holdSession(tab.targetId);
        world.openDialog(tab.targetId);
        await settleRefusal(world, tab, lines);
        await until(() => world.detached().includes(first), 'the first detach');
        await assert.rejects(browser.eval(tab, '1'), /may not answer/);
        world.open.delete(tab.targetId);
        target.url = FORM;
        target.history = {
          currentIndex: 0,
          entries: [{ id: 1, url: SEVEN, title: '', transitionType: 'typed' }],
        };
        assert.equal(await browser.eval(tab, '2'), 'fake-eval');
        const second = world.holdSession(tab.targetId);
        assert.notEqual(second, first);
        world.openDialog(tab.targetId);
        await settleRefusal(world, tab, lines, 1);
        await until(() => world.detached().includes(second), 'the second detach');
      })
    );
  } finally {
    await world.fake.close();
  }
});

test('a main-frame navigation to a protected page detaches the hold; a child frame one does not', async () => {
  const world = await dialogFake();
  try {
    await warnings(() =>
      explicit(world.fake, async () => {
        const tab = await browser.ensureTab(FORM);
        await browser.eval(tab, '1');
        const session = world.holdSession(tab.targetId);
        const frame = (id, url, parentId) => ({
          type: 'Navigation',
          frame: { id, url, loaderId: `L-${id}`, securityOrigin: new URL(url).origin, parentId },
        });
        world.emit(session, 'Page.frameNavigated', frame('CHILD', SEVEN, tab.targetId));
        world.emit(
          session,
          'Page.frameNavigated',
          frame(tab.targetId, 'https://example.test/next')
        );
        await idle();
        assert.ok(!world.detached().includes(session));
        world.emit(session, 'Page.frameNavigated', frame(tab.targetId, SEVEN));
        await until(() => world.detached().includes(session), 'the detach');
        world.openDialog(tab.targetId, {}, session);
        await idle();
        assert.deepEqual(world.handled(), []);
        assert.deepEqual(browser.dialogs(tab), []);
      })
    );
  } finally {
    await world.fake.close();
  }
});

test('a lost dialog connection is held again on the next call while the main connection stays open', async () => {
  const world = await dialogFake();
  try {
    await warnings(() =>
      explicit(world.fake, async () => {
        const tab = await browser.ensureTab(FORM);
        await browser.eval(tab, '1');
        const sub = await browser.websocket.on(tab).forward({ sink: 'log' });
        const first = world.holdSession(tab.targetId);
        const disconnects = world.disconnects.length;
        world.closes.get(first)(1001, 'gone');
        await until(() => world.disconnects.length > disconnects, 'the dialog connection to drop');
        assert.equal(await browser.eval(tab, '2'), 'fake-eval');
        await until(() => world.holdSessions(tab.targetId).length === 2, 'a new hold');
        world.openDialog(tab.targetId, { message: 'held again' });
        await until(() => world.handled().length === 1, 'the answer');
        await sub.close();
      })
    );
  } finally {
    await world.fake.close();
  }
});

test('an answer that crossed a navigation is reported without its message', async () => {
  const world = await dialogFake({
    hold(message, send) {
      if (message.method === 'Page.handleJavaScriptDialog') {
        send({
          method: 'Page.frameNavigated',
          sessionId: message.sessionId,
          params: {
            type: 'Navigation',
            frame: {
              id: world.fake.calls.find((call) => call.sessionId === message.sessionId)?.target,
              url: FORM,
              loaderId: 'L-moved',
              securityOrigin: new URL(FORM).origin,
            },
          },
        });
      }
      return false;
    },
  });
  try {
    const lines = await warnings(() =>
      explicit(world.fake, async () => {
        const tab = await browser.ensureTab(FORM);
        await browser.eval(tab, '1');
        world.openDialog(tab.targetId);
        await until(() => world.handled().length === 1, 'the answer');
        assert.equal(await browser.eval(tab, '2'), 'fake-eval');
        const [record] = browser.dialogs(tab);
        assert.deepEqual(Object.keys(record).sort(), [
          'accepted',
          'at',
          'by',
          'targetId',
          'unsure',
        ]);
        assert.equal(record.accepted, true);
      })
    );
    assert.equal(lines.length, 1, lines.join('\n'));
    assert.match(lines[0], /which dialog the answer reached isn't known/);
    assert.ok(!lines[0].includes(SECRET));
  } finally {
    await world.fake.close();
  }
});

test('a dialog that goes stale during the check is not answered', async () => {
  let release;
  const world = await dialogFake({
    hold(message, send) {
      if (message.method !== 'Target.getTargetInfo' || !release) return false;
      const reply = release;
      release = undefined;
      reply(() =>
        send({
          id: message.id,
          result: {
            targetInfo: { targetId: message.params.targetId, type: 'page', url: FORM },
          },
        })
      );
      return true;
    },
  });
  try {
    await warnings(() =>
      explicit(world.fake, async () => {
        const tab = await browser.ensureTab(FORM);
        await browser.eval(tab, '1');
        const held = Promise.withResolvers();
        release = held.resolve;
        world.openDialog(tab.targetId);
        const answer = await held.promise;
        world.emit(world.holdSession(tab.targetId), 'Page.frameStartedNavigating', {
          frameId: tab.targetId,
        });
        answer();
        await idle();
        assert.deepEqual(world.handled(), []);
        assert.deepEqual(browser.dialogs(tab), []);
      })
    );
  } finally {
    await world.fake.close();
  }
});

test('a detached dialog session is dropped and ignored, and the last drop closes its connection', async () => {
  const world = await dialogFake();
  try {
    await warnings(() =>
      explicit(world.fake, async () => {
        const tab = await browser.ensureTab(FORM);
        const other = await browser.ensureTab('https://other.test/');
        await browser.eval(tab, '1');
        await browser.eval(other, '1');
        await idle();
        const disconnects = world.disconnects.length;
        const session = world.holdSession(tab.targetId);
        const send = world.sends.get(session);
        send({ method: 'Target.detachedFromTarget', params: { sessionId: session } });
        await until(
          () => world.fake.calls.some((call) => call.method === 'Target.detachFromTarget'),
          'the detach'
        );
        assert.equal(world.disconnects.length, disconnects, 'the other tab keeps the connection');
        send({
          method: 'Page.javascriptDialogOpening',
          sessionId: session,
          params: { url: FORM, message: SECRET, type: 'alert', frameId: tab.targetId },
        });
        await idle();
        assert.deepEqual(world.handled(), []);
        assert.deepEqual(browser.dialogs(tab), []);
        const last = world.holdSession(other.targetId);
        send({ method: 'Target.detachedFromTarget', params: { sessionId: last } });
        await until(() => world.disconnects.length === disconnects + 1, 'the connection to close');
      })
    );
  } finally {
    await world.fake.close();
  }
});

test('a discovered endpoint holds dialogs on an ordinary tab and refuses protected ones', async () => {
  const world = await dialogFake();
  const bin = path.join(scratch, 'bin');
  const home = path.join(scratch, 'home');
  const body = JSON.stringify({
    schema: 1,
    minChromium: 116,
    entries: [
      {
        id: 'cdp',
        kind: 'endpoint',
        tier: 1,
        name: 'Chrome',
        endpoint: {
          cdp: `http://127.0.0.1:${world.fake.port}`,
          webSocketDebuggerUrl: world.fake.url,
          name: 'Chrome',
          source: 'flag',
          sevenOpen: true,
          sevenTabs: [],
        },
      },
    ],
  });
  await mkdir(bin, { recursive: true });
  await writeFile(path.join(bin, 'slicc'), `#!/bin/sh\nprintf '%s' '${body}'\n`, { mode: 0o755 });
  const saved = { url: process.env.SLICC_CDP_URL, path: process.env.PATH, home: process.env.HOME };
  delete process.env.SLICC_CDP_URL;
  process.env.PATH = `${bin}${path.delimiter}/nonexistent`;
  process.env.HOME = home;
  try {
    await warnings(async () => {
      const tab = await browser.ensureTab(FORM);
      await browser.eval(tab, '1');
      world.openDialog(tab.targetId, { message: 'ordinary' });
      await until(() => world.handled().length === 1, 'the answer');
      await assert.rejects(browser.ensureTab(SEVEN), /refused/);
      await assert.rejects(
        browser.onDialog({ targetId: 'NOPE' }, () => undefined),
        /refused/
      );
    });
  } finally {
    await idle();
    if (saved.url === undefined) delete process.env.SLICC_CDP_URL;
    else process.env.SLICC_CDP_URL = saved.url;
    process.env.PATH = saved.path;
    process.env.HOME = saved.home;
    await world.fake.close();
  }
});

test('a script that holds a tab ends by itself, after the holder answered between its evals', async () => {
  const world = await dialogFake();
  const script = path.join(scratch, 'script.mjs');
  await writeFile(
    script,
    `import browser from ${JSON.stringify(path.join(root, 'packages/sliccy-browser/src/browser.js'))};
const tab = await browser.ensureTab(${JSON.stringify(FORM)});
await browser.eval(tab, '1');
console.log('ready ' + tab.targetId);
await new Promise((resolve) => process.stdin.once('data', resolve));
process.stdin.pause();
console.log(JSON.stringify({ value: await browser.eval(tab, '2'), dialogs: browser.dialogs(tab) }));
`
  );
  try {
    const child = spawn(process.execPath, [script], {
      env: { ...process.env, SLICC_CDP_URL: world.fake.url },
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    let stdout = '';
    let stderr = '';
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
    });
    const exited = new Promise((resolve) => child.on('close', (code) => resolve(code)));
    const ready = Promise.withResolvers();
    child.stdout.on('data', (chunk) => {
      stdout += chunk;
      const match = stdout.match(/^ready (\S+)\n/);
      if (match) ready.resolve(match[1]);
    });
    const targetId = await Promise.race([
      ready.promise,
      exited.then((code) => {
        throw new Error(`child exited ${code}: ${stderr}`);
      }),
    ]);
    await idle();
    world.openDialog(targetId, { message: 'between evals' });
    await until(() => world.handled().length === 1, 'the answer');
    child.stdin.write('go\n');
    const timer = setTimeout(() => child.kill('SIGKILL'), 5000);
    const code = await exited;
    clearTimeout(timer);
    assert.equal(code, 0, stderr);
    const result = JSON.parse(stdout.split('\n')[1]);
    assert.equal(result.value, 'fake-eval');
    assert.deepEqual(
      result.dialogs.map(({ type, message, accepted }) => [type, message, accepted]),
      [['alert', 'between evals', true]]
    );
    assert.match(stderr, /alert dialog nobody answered, so it was accepted: "between evals"/);
  } finally {
    await world.fake.close();
  }
});
