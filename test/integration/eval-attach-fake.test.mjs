import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { evalBackend } from './eval-backend.mjs';
import { evalCommand as command, evalHolder as holder } from './eval-support.mjs';
import { kernelRunner } from './runners.mjs';
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

const installs = (call) => call.params?.expression?.startsWith('(() => { const queue = [];');
const worldIndex = (calls, frameId) =>
  calls.findLastIndex(
    (call) => call.method === 'Page.createIsolatedWorld' && call.params.frameId === frameId
  );
const admissionReads = 1;
const holderTreeRead = (frameId, nth) => (message, calls) => {
  const start = worldIndex(calls, frameId);
  if (start < 0 || message.method !== 'Page.getFrameTree') return false;
  if (message.sessionId !== calls[start].sessionId) return false;
  const reads = calls
    .slice(start)
    .filter((call) => call.method === 'Page.getFrameTree' && call.sessionId === message.sessionId);
  return reads.length === admissionReads + nth;
};
const trace = (calls, frameId) =>
  calls
    .slice(Math.max(0, worldIndex(calls, frameId)))
    .slice(0, 40)
    .map((call) => `${call.method}@${call.sessionId ?? 'browser'}`)
    .join(' ');
const contextWithoutUnique = (message, send, isDefault) => {
  send({
    method: 'Runtime.executionContextCreated',
    sessionId: message.sessionId,
    params: {
      context: {
        id: isDefault ? 1 : 999,
        origin: 'https://example.test',
        name: '',
        auxData: { frameId: 'TAB', isDefault },
      },
    },
  });
  send({
    id: message.id,
    sessionId: message.sessionId,
    result: isDefault ? {} : { executionContextId: 999 },
  });
  return true;
};
const sites = {
  'default-no-uniqueId': () => ({
    intercept: (message, send) =>
      message.method === 'Runtime.enable' && contextWithoutUnique(message, send, true),
  }),
  'create-world-failed': () => ({ refuse: ['Page.createIsolatedWorld'] }),
  'context-not-absorbed': () => ({
    intercept(message, send) {
      if (message.method !== 'Page.createIsolatedWorld') return false;
      send({ id: message.id, sessionId: message.sessionId, result: { executionContextId: 999 } });
      return true;
    },
  }),
  'isolated-no-uniqueId': () => ({
    intercept: (message, send) =>
      message.method === 'Page.createIsolatedWorld' && contextWithoutUnique(message, send, false),
  }),
  'tree-read-failed': () => ({ refuseWhen: holderTreeRead('TAB', 1) }),
  'tree-mismatch': (tab) => ({
    onCommand(message) {
      if (message.method === 'Page.createIsolatedWorld') tab.url = 'https://example.test/moved';
    },
  }),
  'chain-check-failed': () => ({ refuseWhen: holderTreeRead('SAME', 2) }),
};

for (const [site, setup] of Object.entries(sites)) {
  test(`eval-attach fake isolated: unverified-context refusal names ${site}`, async () => {
    const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-site-'));
    const chained = site === 'chain-check-failed';
    const frameId = chained ? 'SAME' : 'TAB';
    const same = {
      id: 'SAME',
      parentId: 'TAB',
      loaderId: 'SAME-L',
      securityOrigin: 'https://example.test',
      url: 'https://example.test/same',
    };
    const tab = {
      targetId: 'TAB',
      url: 'https://example.test/top',
      loaderId: 'L',
      ...(chained ? { childFrames: [{ frame: same }] } : {}),
    };
    const backend = await evalBackend(setup(tab));
    backend.targets.push(tab);
    await writeFile(path.join(home, 'shim.js'), 'send("installed");');
    const held = holder(
      backend.url,
      [
        'eval-attach',
        'shim.js',
        '--tab',
        'TAB',
        '--world',
        'isolated',
        ...(chained ? ['--frames', 'same-origin'] : []),
      ],
      home
    );
    try {
      const refused = await held.event('refused');
      assert.equal(refused.reason, 'unverified-context');
      assert.equal(refused.site, site, trace(backend.calls, frameId));
      assert.equal(refused.frameId, frameId);
      const diagnosed = site === 'create-world-failed' || site === 'tree-read-failed';
      if (diagnosed) {
        assert.equal(refused.cause, 'other');
        assert.equal(refused.frame, 'equal');
      } else {
        assert.equal(refused.cause, undefined);
        assert.equal(refused.frame, undefined);
      }
      assert.deepEqual(Object.keys(refused).sort(), [
        'attachmentId',
        ...(diagnosed ? ['cause'] : []),
        'event',
        ...(diagnosed ? ['frame'] : []),
        'frameId',
        ...(chained ? ['origin', 'parentFrameId'] : []),
        'reason',
        'site',
        'tab',
      ]);
      assert.ok(held.output().stdout.includes(`"reason":"unverified-context","site":"${site}"`));
      await new Promise((resolve) => setTimeout(resolve, 600));
      held.child.stdin.end();
      assert.equal((await held.ended).code, 0);
      assert.equal(held.events().filter((event) => event.event === 'refused').length, 1);
      assert.ok(
        !held
          .events()
          .some(
            (event) =>
              ['document-ready', 'message'].includes(event.event) && event.frameId === frameId
          )
      );
      assert.ok(
        !backend.calls.some(
          (call) => installs(call) && (!chained || call.params.uniqueContextId?.includes(':SAME:'))
        )
      );
    } finally {
      held.child.kill('SIGKILL');
      await backend.close();
      await rm(home, { recursive: true, force: true });
    }
  });
}

test('eval-attach fake isolated: a refusal with another reason names no site', async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-site-'));
  const backend = await evalBackend();
  backend.targets.push({ targetId: 'TAB', url: 'https://example.test/top', loaderId: 'L' });
  await writeFile(path.join(home, 'shim.js'), 'throw new Error("body");');
  const held = holder(
    backend.url,
    ['eval-attach', 'shim.js', '--tab', 'TAB', '--world', 'isolated'],
    home
  );
  try {
    const refused = await held.event('refused');
    assert.equal(refused.reason, 'installation');
    assert.equal(refused.cause, undefined);
    assert.equal(refused.frame, undefined);
    assert.deepEqual(Object.keys(refused).sort(), [
      'attachmentId',
      'event',
      'frameId',
      'reason',
      'tab',
    ]);
    held.child.stdin.end();
    assert.equal((await held.ended).code, 0);
  } finally {
    held.child.kill('SIGKILL');
    await backend.close();
    await rm(home, { recursive: true, force: true });
  }
});

const diagnosedKeys = [
  'attachmentId',
  'cause',
  'event',
  'frame',
  'frameId',
  'reason',
  'site',
  'tab',
];

const causeRows = {
  'guard-refused': {
    site: 'create-world-failed',
    setup: () => ({
      refuse: ['Page.createIsolatedWorld'],
      refusalMessage: 'slicc: refused: that execution context belongs to a browser extension',
    }),
  },
  'nested-read-timeout': {
    site: 'create-world-failed',
    setup: () => ({
      refuse: ['Page.createIsolatedWorld'],
      refusalMessage: 'Page.createIsolatedWorld timed out',
    }),
  },
  transport: {
    site: 'create-world-failed',
    setup: () => ({
      intercept(message, send, close) {
        if (message.method !== 'Page.createIsolatedWorld') return false;
        close(1006, 'gone');
        return true;
      },
    }),
  },
  other: {
    site: 'create-world-failed',
    setup: () => ({ refuse: ['Page.createIsolatedWorld'] }),
  },
};

for (const [cause, { site, setup }] of Object.entries(causeRows)) {
  test(`eval-attach fake isolated: create-world-failed cause is ${cause}`, async () => {
    const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-cause-'));
    const backend = await evalBackend(setup());
    backend.targets.push({ targetId: 'TAB', url: 'https://example.test/top', loaderId: 'L' });
    await writeFile(path.join(home, 'shim.js'), 'send("installed");');
    const held = holder(
      backend.url,
      ['eval-attach', 'shim.js', '--tab', 'TAB', '--world', 'isolated'],
      home
    );
    try {
      const refused = await held.event('refused');
      assert.equal(refused.reason, 'unverified-context');
      assert.equal(refused.site, site);
      assert.equal(refused.cause, cause, trace(backend.calls, 'TAB'));
      assert.equal(refused.frame, 'equal');
      assert.deepEqual(Object.keys(refused).sort(), diagnosedKeys);
      assert.ok(
        !backend.calls.some((call) => installs(call)),
        'refused create-world must not install'
      );
    } finally {
      held.child.kill('SIGKILL');
      await backend.close();
      await rm(home, { recursive: true, force: true });
    }
  });
}

test('eval-attach fake isolated: tree-read-failed carries cause and frame', async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-tree-cause-'));
  const backend = await evalBackend({
    refuseWhen: holderTreeRead('TAB', 1),
    refusalMessage:
      'slicc: refused: the page changed while slicc was checking it; try again once it settles',
  });
  backend.targets.push({ targetId: 'TAB', url: 'https://example.test/top', loaderId: 'L' });
  await writeFile(path.join(home, 'shim.js'), 'send("installed");');
  const held = holder(
    backend.url,
    ['eval-attach', 'shim.js', '--tab', 'TAB', '--world', 'isolated'],
    home
  );
  try {
    const refused = await held.event('refused');
    assert.equal(refused.reason, 'unverified-context');
    assert.equal(refused.site, 'tree-read-failed');
    assert.equal(refused.cause, 'guard-refused');
    assert.equal(refused.frame, 'equal');
    assert.deepEqual(Object.keys(refused).sort(), diagnosedKeys);
  } finally {
    held.child.kill('SIGKILL');
    await backend.close();
    await rm(home, { recursive: true, force: true });
  }
});

const frameRows = {
  equal: {
    onWorld(message, send) {
      send({
        id: message.id,
        sessionId: message.sessionId,
        error: { code: -32000, message: 'Page.createIsolatedWorld refused' },
      });
    },
  },
  changed: {
    onWorld(message, send) {
      send({
        method: 'Page.frameNavigated',
        sessionId: message.sessionId,
        params: {
          frame: {
            id: 'TAB',
            loaderId: 'MOVED',
            url: 'https://example.test/moved',
            securityOrigin: 'https://example.test',
          },
        },
      });
      send({
        id: message.id,
        sessionId: message.sessionId,
        error: { code: -32000, message: 'Page.createIsolatedWorld refused' },
      });
    },
  },
  sibling: {
    onWorld(message, send) {
      send({
        method: 'Page.frameAttached',
        sessionId: message.sessionId,
        params: { frameId: 'CHILD', parentFrameId: 'TAB' },
      });
      send({
        id: message.id,
        sessionId: message.sessionId,
        error: { code: -32000, message: 'Page.createIsolatedWorld refused' },
      });
    },
  },
  absent: {
    onWorld(message, send) {
      send({
        method: 'Target.targetDestroyed',
        params: { targetId: 'TAB' },
      });
      send({
        id: message.id,
        sessionId: message.sessionId,
        error: { code: -32000, message: 'Page.createIsolatedWorld refused' },
      });
    },
  },
};

for (const [relation, { onWorld }] of Object.entries(frameRows)) {
  test(`eval-attach fake isolated: create-world-failed frame is ${relation}`, async () => {
    const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-frame-'));
    const backend = await evalBackend({
      intercept(message, send) {
        if (message.method !== 'Page.createIsolatedWorld') return false;
        onWorld(message, send);
        return true;
      },
    });
    backend.targets.push({ targetId: 'TAB', url: 'https://example.test/top', loaderId: 'L' });
    await writeFile(path.join(home, 'shim.js'), 'send("installed");');
    const held = holder(
      backend.url,
      ['eval-attach', 'shim.js', '--tab', 'TAB', '--world', 'isolated'],
      home
    );
    try {
      const refused = await held.event('refused');
      assert.equal(refused.reason, 'unverified-context');
      assert.equal(refused.site, 'create-world-failed');
      if (relation !== 'absent') assert.equal(refused.cause, 'other');
      else assert.ok(typeof refused.cause === 'string' && refused.cause.length > 0);
      assert.equal(refused.frame, relation, trace(backend.calls, 'TAB'));
      assert.deepEqual(Object.keys(refused).sort(), diagnosedKeys);
    } finally {
      held.child.kill('SIGKILL');
      await backend.close();
      await rm(home, { recursive: true, force: true });
    }
  });
}

test('eval-attach fake isolated: refused create-world CDP sequence stays counting-identical', async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-count-'));
  const backend = await evalBackend({ refuse: ['Page.createIsolatedWorld'] });
  backend.targets.push({ targetId: 'TAB', url: 'https://example.test/top', loaderId: 'L' });
  await writeFile(path.join(home, 'shim.js'), 'send("installed");');
  const held = holder(
    backend.url,
    ['eval-attach', 'shim.js', '--tab', 'TAB', '--world', 'isolated'],
    home
  );
  try {
    await held.event('refused');
    const start = worldIndex(backend.calls, 'TAB');
    assert.ok(start >= 0);
    assert.equal(backend.calls[start].method, 'Page.createIsolatedWorld');
    const untilTick = backend.calls
      .slice(start + 1)
      .findIndex((call) => call.method === 'Page.getFrameTree');
    const pass =
      untilTick < 0
        ? backend.calls.slice(start + 1)
        : backend.calls.slice(start + 1, start + 1 + untilTick);
    assert.deepEqual(
      pass.map((call) => call.method),
      [],
      'refused create-world pass must not add CDP calls before the next tick tree read'
    );
    assert.ok(!backend.calls.some((call) => installs(call)));
    held.child.stdin.end();
    assert.equal((await held.ended).code, 0);
  } finally {
    held.child.kill('SIGKILL');
    await backend.close();
    await rm(home, { recursive: true, force: true });
  }
});

const worldCalls = (calls) => calls.filter((call) => call.method === 'Page.createIsolatedWorld');
const moveFrames = (message, send) => {
  for (const [method, params] of [
    ['Page.frameAttached', { frameId: 'KID', parentFrameId: 'TAB' }],
    ['Page.frameDetached', { frameId: 'KID', reason: 'remove' }],
  ])
    send({ method, params, sessionId: message.sessionId });
  return false;
};
const retryRows = {
  'a refusal while a child frame moves is retried and installs once': {
    options: () => ({
      intercept: (message, send, _close, calls) =>
        message.method === 'Page.createIsolatedWorld' &&
        worldCalls(calls).length === 1 &&
        moveFrames(message, send),
    }),
    worlds: 2,
    installed: ['world:TAB:L'],
  },
  'a refusal that persists while the frame moves stops after three attempts': {
    options: () => ({
      intercept: (message, send) =>
        message.method === 'Page.createIsolatedWorld' && moveFrames(message, send),
    }),
    worlds: 3,
    cause: 'guard-refused',
    frame: 'sibling',
    installed: [],
  },
  'a backend error while the frame moves is final on the first attempt': {
    options: () => ({
      intercept(message, send) {
        if (message.method !== 'Page.createIsolatedWorld') return false;
        moveFrames(message, send);
        send({
          id: message.id,
          sessionId: message.sessionId,
          error: { code: -32000, message: 'Cannot create world' },
        });
        return true;
      },
    }),
    worlds: 1,
    cause: 'other',
    frame: 'sibling',
    installed: [],
  },
  'a retry stops when the document context ends while the frame moves': {
    options: () => ({
      intercept(message, send, _close, calls) {
        if (message.method !== 'Page.createIsolatedWorld') return false;
        moveFrames(message, send);
        if (worldCalls(calls).length === 2)
          send({
            method: 'Runtime.executionContextDestroyed',
            params: { executionContextId: 1 },
            sessionId: message.sessionId,
          });
        return false;
      },
    }),
    worlds: 2,
    cause: 'guard-refused',
    frame: 'changed',
    installed: [],
  },
  'a refusal on a stable frame is final on the first attempt': {
    options: () => ({ refuseWhen: holderTreeRead('TAB', 0) }),
    worlds: 1,
    cause: 'guard-refused',
    frame: 'equal',
    installed: [],
  },
  'a navigation while a retry is pending gives the next document a fresh budget': {
    options: (tab, navigate) => {
      let navigated = false;
      return {
        intercept(message, send, _close, calls) {
          if (message.method === 'Page.createIsolatedWorld' && worldCalls(calls).length === 1)
            return moveFrames(message, send);
          if (
            !navigated &&
            worldCalls(calls).length === 1 &&
            holderTreeRead('TAB', 1)(message, calls)
          ) {
            navigated = true;
            navigate(tab, 'https://example.test/next', 'NEXT');
          }
          return false;
        },
      };
    },
    worlds: 2,
    installed: ['world:TAB:NEXT'],
  },
};
const runtimes = {
  async native(backend, args) {
    const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-retry-'));
    await writeFile(path.join(home, 'shim.js'), 'send("installed");');
    const held = holder(backend.url, ['eval-attach', 'shim.js', ...args], home);
    return {
      events: async () => held.events(),
      async stop() {
        held.child.stdin.end();
        assert.equal((await held.ended).code, 0);
      },
      async close() {
        held.child.kill('SIGKILL');
        await rm(home, { recursive: true, force: true });
      },
    };
  },
  async wasi(backend, args) {
    const runner = await kernelRunner(
      path.resolve(process.env.SLICC_EVAL_WASM || 'target/wasm32-wasip1/debug/playwright-cli.wasm'),
      path.resolve('target/wasm32-wasip1/debug/curlwright.wasm'),
      backend.url
    );
    try {
      await runner.install('@ai-ecoverse/wasm-bash');
      await runner.install('@ai-ecoverse/wasm-coreutils');
      const cwd = await runner.fresh();
      await runner.write(cwd, 'shim.js', 'send("installed");');
      const started = await runner.run(
        [
          'bash',
          '-c',
          `(while sleep 1; do echo || break; done) | playwright-cli -s retry eval-attach shim.js ${args.join(' ')} > eval.log 2> eval.err &`,
        ],
        cwd
      );
      assert.equal(started.code, 0, started.stderr);
      const events = async () => {
        const data = await runner.read(cwd, 'eval.log').catch(() => new Uint8Array());
        return new TextDecoder()
          .decode(data)
          .split('\n')
          .filter((line) => line.startsWith('{'))
          .map(JSON.parse);
      };
      return {
        events,
        async stop() {
          const ready = (await events()).find((event) => event.event === 'ready');
          const detached = await runner.playwright(
            ['-s', 'retry', 'eval-detach', ready.attachmentId],
            cwd
          );
          assert.equal(detached.code, 0, detached.stderr);
          await until(async () => (await events()).some((event) => event.event === 'stopped'));
        },
        async close() {
          runner.close();
        },
      };
    } catch (error) {
      runner.close();
      throw error;
    }
  },
};
const until = async (check, ms = 30000) => {
  const end = Date.now() + ms;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() >= end) throw new Error('eval holder timed out');
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
};

for (const [runtime, start] of Object.entries(runtimes)) {
  for (const [name, row] of Object.entries(retryRows)) {
    test(`eval-attach fake isolated ${runtime}: ${name}`, async () => {
      const tab = { targetId: 'TAB', url: 'https://example.test/top', loaderId: 'L' };
      let backend;
      const options = row.options(tab, (...args) => backend.navigate(...args));
      const intercept = options.intercept;
      backend = await evalBackend({
        ...options,
        ...(intercept
          ? { intercept: (message, send, close) => intercept(message, send, close, backend.calls) }
          : {}),
      });
      backend.targets.push(tab);
      let held;
      try {
        held = await start(backend, ['--tab', 'TAB', '--world', 'isolated']);
        const refusals = async () =>
          (await held.events()).filter((event) => event.event === 'refused');
        if (row.installed.length) {
          await until(async () => (await held.events()).some((event) => event.event === 'message'));
        } else {
          const refused = await until(async () => (await refusals())[0]);
          assert.equal(refused.reason, 'unverified-context');
          assert.equal(refused.site, 'create-world-failed', trace(backend.calls, 'TAB'));
          assert.equal(refused.cause, row.cause, trace(backend.calls, 'TAB'));
          assert.equal(refused.frame, row.frame, trace(backend.calls, 'TAB'));
          assert.deepEqual(Object.keys(refused).sort(), diagnosedKeys);
        }
        await new Promise((resolve) => setTimeout(resolve, 800));
        await held.stop();
        assert.equal(worldCalls(backend.calls).length, row.worlds, trace(backend.calls, 'TAB'));
        assert.deepEqual(
          backend.calls.filter(installs).map((call) => call.params.uniqueContextId),
          row.installed
        );
        assert.equal((await refusals()).length, row.installed.length ? 0 : 1);
        const ready = (await held.events()).filter((event) => event.event === 'document-ready');
        assert.deepEqual(
          ready.map((event) => event.loaderId),
          row.installed.map((unique) => unique.split(':').at(-1))
        );
      } finally {
        await held?.close();
        await backend.close();
      }
    });
  }
}
