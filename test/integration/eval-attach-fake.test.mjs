import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { evalBackend } from './eval-backend.mjs';
import { evalCommand as command, evalHolder as holder } from './eval-support.mjs';
import './watchdog.mjs';

for (const world of ['main', 'isolated']) {
  for (const mode of ['owned', 'missed', 'miss-first', 'resumed', 'scriptless']) {
    const missPause = mode !== 'owned';
    test(`eval-attach early fake ${world}: ${mode} pause`, async () => {
      const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-early-fake-'));
      const backend = await evalBackend({
        missPause: mode === 'missed',
        noScripts: mode === 'scriptless',
        missFirstPause: mode === 'miss-first',
        resumeBeforeEval: mode === 'resumed',
      });
      const tab = { targetId: 'TAB', url: 'https://example.test/initial', loaderId: 'L' };
      backend.targets.push(tab);
      await writeFile(path.join(home, 'shim.js'), 'send({arbitrary: "early source"});');
      const held = holder(
        backend.url,
        ['eval-attach', 'shim.js', '--tab', 'TAB', '--world', world, '--early'],
        home
      );
      try {
        assert.equal((await held.event('ready')).early, true);
        await held.event('refused', (event) => event.reason === 'late');
        backend.navigate(tab, 'https://example.test/next', 'NEXT');
        if (missPause) {
          await held.event(
            'refused',
            () => held.events().filter((event) => event.event === 'refused').length === 2
          );
          assert.equal(held.events().filter((event) => event.event === 'document-ready').length, 0);
        } else {
          const ready = await held.event('document-ready');
          assert.equal(ready.early, true);
          assert.equal(ready.loaderId, 'NEXT');
          assert.deepEqual((await held.event('message')).payload, { arbitrary: 'early source' });
        }
        held.child.stdin.end();
        assert.equal((await held.ended).code, 0);
        const installs = backend.calls.filter((call) =>
          call.params?.expression?.startsWith('(() => { const queue = [];')
        );
        assert.equal(installs.length, missPause ? 0 : 1);
        assert.equal(
          backend.calls.filter(
            (call) => call.method === 'Slicc.action' && call.params.phase === 'start'
          ).length,
          mode === 'resumed' ? 1 : installs.length
        );
        assert.ok(
          !backend.calls.some((call) =>
            ['Page.addScriptToEvaluateOnNewDocument', 'Runtime.addBinding'].includes(call.method)
          )
        );
      } finally {
        held.child.kill('SIGKILL');
        await backend.close();
        await rm(home, { recursive: true, force: true });
      }
    });
  }
}

for (const world of ['main', 'isolated']) {
  test(`eval-attach fake ${world}: navigation, new tabs, scope misses, send and detach`, async () => {
    const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-fake-'));
    const backend = await evalBackend();
    const tab = { targetId: 'TAB', url: 'https://example.test/a', loaderId: 'L' };
    backend.targets.push(tab, { targetId: 'MISS', url: 'https://other.test/', loaderId: 'M' });
    await writeFile(
      path.join(home, 'shim.js'),
      'send({href: location.href, data: "line\\n\\u009b"});'
    );
    const held = holder(
      backend.url,
      ['eval-attach', 'shim.js', '--host', '*.example.test', '--world', world],
      home
    );
    try {
      const ready = await held.event('ready');
      assert.match(ready.attachmentId, /^eval-[a-f0-9]{24}$/);
      const first = await held.event('document-ready');
      assert.equal(first.early, false);
      assert.equal(first.world, world);
      assert.equal(first.frameId, 'TAB');
      assert.equal(first.loaderId, 'L');
      const message = await held.event('message');
      assert.equal(message.payload.href, tab.url);
      assert.equal(message.payload.data, 'line\n\u009b');
      backend.navigate(tab, 'https://example.test/a/b', 'NEXT');
      await held.event('document-ready', (event) => event.loaderId === 'NEXT');
      await held.event('message', (event) => event.loaderId === 'NEXT');
      backend.targets.push({ targetId: 'NEW', url: 'https://x.example.test/a', loaderId: 'NEW-L' });
      await held.event('message', (event) => event.tab === 'NEW');
      assert.ok(!held.events().some((event) => event.tab === 'MISS'));
      assert.equal(
        held.events().filter((event) => event.event === 'document-ready' && event.loaderId === 'L')
          .length,
        1
      );
      const wrong = await command(backend.url, ['eval-detach', 'wrong-id'], home);
      assert.equal(wrong.code, 1);
      const detached = await command(backend.url, ['eval-detach', ready.attachmentId], home);
      assert.equal(detached.code, 0, detached.stderr);
      const result = await held.ended;
      assert.equal(result.code, 0, result.stderr);
      assert.deepEqual(JSON.parse(detached.stdout), held.events().at(-1));
      assert.equal(held.events().at(-1).reason, 'detach');
      const installed = backend.calls.filter((call) =>
        call.params?.expression?.startsWith('(() => { const queue = [];')
      );
      assert.equal(installed.length, 3);
      const actions = backend.calls.filter((call) => call.method === 'Slicc.action');
      assert.equal(actions.length, 6);
      assert.equal(
        actions.filter((call) => call.params.phase === 'start' && call.params.kind === 'eval')
          .length,
        3
      );
      assert.equal(
        actions.filter((call) => call.params.phase === 'end' && call.params.ok === true).length,
        3
      );
      assert.ok(installed.every((call) => typeof call.params.uniqueContextId === 'string'));
      assert.ok(
        backend.calls
          .filter((call) => call.method === 'Target.attachToTarget')
          .every((call) => call.params.flatten === true)
      );
      for (const forbidden of [
        'Page.addScriptToEvaluateOnNewDocument',
        'Runtime.addBinding',
        'Debugger.enable',
        'Debugger.pause',
      ])
        assert.ok(!backend.calls.some((call) => call.method === forbidden), forbidden);
    } finally {
      held.child.kill('SIGKILL');
      await backend.close();
      await rm(home, { recursive: true, force: true });
    }
  });
}

test('eval-attach fake: repeatable URL matching, protected document refusal and stdin shutdown', async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-match-'));
  const backend = await evalBackend();
  const tab = { targetId: 'TAB', url: 'https://example.test/allowed/a', loaderId: 'L' };
  backend.targets.push(tab, { targetId: 'MISS', url: 'https://example.test/miss', loaderId: 'M' });
  await writeFile(path.join(home, 'shim.js'), 'send("hello");');
  const held = holder(
    backend.url,
    [
      'eval-attach',
      'shim.js',
      '--match',
      '*://example.test/allowed/*',
      '--match',
      'https://other.test/*',
    ],
    home
  );
  try {
    await held.event('message');
    backend.targets.push({ targetId: 'NEW', url: 'https://other.test/a', loaderId: 'N' });
    await held.event('message', (event) => event.tab === 'NEW');
    assert.ok(!held.events().some((event) => event.tab === 'MISS'));
    held.child.stdin.end();
    assert.equal((await held.ended).code, 0);
    assert.equal(held.events().at(-1).reason, 'stdin');
    const protectedHolder = holder(backend.url, ['eval-attach', 'shim.js', '--tab', 'TAB'], home);
    try {
      await protectedHolder.event('document-ready');
      backend.navigate(tab, 'https://vault.sliccy.ai/SECRET?q=SECRET', 'PROTECTED');
      const refused = await protectedHolder.event('refused');
      assert.equal(refused.tab, 'TAB');
      assert.equal(refused.url, undefined);
      assert.ok(!JSON.stringify(refused).includes('SECRET'));
      protectedHolder.child.stdin.end();
      assert.equal((await protectedHolder.ended).code, 0);
      assert.equal(protectedHolder.events().filter((event) => event.event === 'refused').length, 1);
      assert.ok(
        !backend.calls.some((call) => call.params?.uniqueContextId === 'default:TAB:PROTECTED')
      );
    } finally {
      protectedHolder.child.kill('SIGKILL');
    }
  } finally {
    held.child.kill('SIGKILL');
    await backend.close();
    await rm(home, { recursive: true, force: true });
  }
});

test('eval-attach precheck rejects protected scopes and child frames', async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-precheck-'));
  try {
    for (const flags of [
      ['--host', '*.sliccy.ai'],
      ['--match', 'https://seven.sliccy.ai/*'],
      ['--tab', 'T', '--frame', 'F'],
      ['--tab', 'T', '--host', 'example.test'],
      ['--tab', 'T', '--world', 'unknown'],
    ]) {
      const result = await command(
        'http://127.0.0.1:1',
        ['eval-attach', 'shim.js', ...flags],
        home
      );
      assert.equal(result.code, 1);
      assert.ok(!result.stderr.includes('Connection refused'), result.stderr);
    }
  } finally {
    await rm(home, { recursive: true, force: true });
  }
});

test('eval-attach fake: child targets and thrown bodies are nonfatal refusals, one lease per session', async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-refuse-'));
  const backend = await evalBackend();
  backend.targets.push(
    { targetId: 'TAB', url: 'https://example.test/', loaderId: 'L' },
    {
      targetId: 'CHILD',
      type: 'iframe',
      parentFrameId: 'TAB',
      url: 'https://example.test/child',
      loaderId: 'C',
    }
  );
  await writeFile(path.join(home, 'shim.js'), 'throw new Error("SECRET");');
  let held;
  try {
    held = holder(backend.url, ['eval-attach', 'shim.js', '--tab', 'CHILD'], home);
    const childRefusal = await held.event('refused');
    assert.equal(childRefusal.reason, 'guard');
    assert.ok(
      !backend.calls.some((call) => call.target === 'CHILD' && call.method === 'Runtime.evaluate')
    );
    held.child.stdin.end();
    assert.equal((await held.ended).code, 0);
    held = holder(backend.url, ['eval-attach', 'shim.js', '--tab', 'TAB'], home);
    await held.event('refused', (event) => event.reason === 'installation');
    const competing = await command(backend.url, ['eval-attach', 'shim.js', '--tab', 'TAB'], home);
    assert.equal(competing.code, 1);
    assert.match(competing.stderr, /already held/);
    assert.ok(!JSON.stringify(held.events()).includes('SECRET'));
    held.child.stdin.end();
    assert.equal((await held.ended).code, 0);
    assert.equal(held.events().filter((event) => event.event === 'refused').length, 1);
  } finally {
    held?.child.kill('SIGKILL');
    await backend.close();
    await rm(home, { recursive: true, force: true });
  }
});

test('eval-attach fake isolated: navigation during world creation fails closed and later documents recover', async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-race-'));
  const tab = { targetId: 'TAB', url: 'https://example.test/allowed/a', loaderId: 'L' };
  let race = true;
  let backend;
  backend = await evalBackend({
    onCommand(message) {
      if (race && message.method === 'Page.createIsolatedWorld') {
        race = false;
        backend.navigate(tab, 'https://other.test/miss', 'RACE');
      }
    },
  });
  backend.targets.push(tab);
  await writeFile(path.join(home, 'shim.js'), 'send(location.href);');
  const held = holder(
    backend.url,
    ['eval-attach', 'shim.js', '--match', 'https://example.test/allowed/*', '--world', 'isolated'],
    home
  );
  try {
    await held.event('refused');
    assert.ok(
      !backend.calls.some((call) =>
        call.params?.expression?.startsWith('(() => { const queue = [];')
      )
    );
    backend.navigate(tab, 'https://example.test/allowed/recovered', 'RECOVERED');
    await held.event('message', (event) => event.loaderId === 'RECOVERED');
    held.child.stdin.end();
    assert.equal((await held.ended).code, 0);
    assert.ok(!held.events().some((event) => event.loaderId === 'RACE'));
  } finally {
    held.child.kill('SIGKILL');
    await backend.close();
    await rm(home, { recursive: true, force: true });
  }
});

test('eval-attach fake main: same-document URL races cannot execute the body outside a match', async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-history-'));
  const tab = { targetId: 'TAB', url: 'https://example.test/allowed/a', loaderId: 'L' };
  let backend;
  let raced = false;
  backend = await evalBackend({
    intercept(message) {
      if (!raced && message.params?.expression?.startsWith('(() => { const queue = [];')) {
        raced = true;
        tab.url = 'https://example.test/miss';
      }
      return false;
    },
  });
  backend.targets.push(tab);
  await writeFile(path.join(home, 'shim.js'), 'globalThis.didInstall = true; send("forbidden");');
  const held = holder(
    backend.url,
    ['eval-attach', 'shim.js', '--match', 'https://example.test/allowed/*'],
    home
  );
  try {
    await held.event('refused');
    assert.equal(raced, true);
    assert.ok([...backend.worlds.values()].every((world) => world.didInstall === undefined));
    held.child.stdin.end();
    assert.equal((await held.ended).code, 0);
    assert.ok(
      !held.events().some((event) => event.event === 'document-ready' || event.event === 'message')
    );
  } finally {
    held.child.kill('SIGKILL');
    await backend.close();
    await rm(home, { recursive: true, force: true });
  }
});
