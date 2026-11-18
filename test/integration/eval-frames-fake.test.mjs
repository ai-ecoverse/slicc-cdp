import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { evalBackend } from './eval-backend.mjs';
import { evalCommand as command, evalHolder as holder } from './eval-support.mjs';
import './watchdog.mjs';

const frame = (id, parentId, origin = 'https://example.test', loaderId = `${id}-L`) => ({
  id,
  parentId,
  loaderId,
  securityOrigin: origin,
  url: `${origin}/${id}`,
});

for (const world of ['main', 'isolated']) {
  test(`eval frames fake ${world}: local, nested, OOPIF, send, calls and child navigation`, async () => {
    const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-frames-'));
    const backend = await evalBackend({ evalCalls: true });
    const same = frame('SAME', 'TAB');
    const nested = frame('NESTED', 'SAME');
    const foreign = frame('FOREIGN', 'TAB', 'https://foreign.test');
    const miss = frame('MISS', 'TAB', 'https://miss.test');
    const tab = {
      targetId: 'TAB',
      url: 'https://example.test/top',
      loaderId: 'L',
      childFrames: [{ frame: same, childFrames: [{ frame: nested }] }, { frame: miss }],
    };
    backend.targets.push(tab, {
      targetId: 'FOREIGN',
      type: 'iframe',
      parentFrameId: 'TAB',
      url: foreign.url,
      loaderId: foreign.loaderId,
      frame: foreign,
      childFrames: [{ frame: frame('DEEP', 'FOREIGN', 'https://foreign.test') }],
    });
    await writeFile(
      path.join(home, 'shim.js'),
      'globalThis.marker = location.href; send({href: location.href});'
    );
    let held;
    try {
      held = holder(
        backend.url,
        [
          'eval-attach',
          'shim.js',
          '--tab',
          'TAB',
          '--world',
          world,
          '--frames',
          'same-origin',
          '--frames',
          'https://foreign.test',
        ],
        home
      );
      const ready = await held.event('ready');
      for (const id of ['SAME', 'NESTED', 'FOREIGN', 'DEEP']) {
        const installed = await held.event('document-ready', (event) => event.frameId === id);
        assert.equal(installed.early, false);
        assert.equal(
          installed.parentFrameId,
          id === 'NESTED' ? 'SAME' : id === 'DEEP' ? 'FOREIGN' : 'TAB'
        );
        assert.equal(
          installed.origin,
          ['FOREIGN', 'DEEP'].includes(id) ? 'https://foreign.test' : 'https://example.test'
        );
        assert.deepEqual(
          installed.ancestors.map((ancestor) => ancestor.frameId),
          id === 'NESTED' ? ['TAB', 'SAME'] : id === 'DEEP' ? ['TAB', 'FOREIGN'] : ['TAB']
        );
        const message = await held.event('message', (event) => event.frameId === id);
        assert.equal(message.payload.href, installed.url);
        assert.deepEqual(message.ancestors, installed.ancestors);
        const called = await command(
          backend.url,
          [
            'eval',
            'globalThis.marker',
            '--tab',
            'TAB',
            '--frame',
            id,
            '--attachment',
            ready.attachmentId,
            '--loader',
            installed.loaderId,
          ],
          home
        );
        assert.equal(called.code, 0, called.stderr);
        assert.ok(called.stdout.includes(installed.url), called.stdout);
      }
      await held.event(
        'refused',
        (event) => event.frameId === 'MISS' && event.reason === 'origin-not-requested'
      );
      assert.ok(!backend.calls.some((call) => call.params?.uniqueContextId?.includes(':MISS:')));
      const previous = await held.event('document-ready', (event) => event.frameId === 'FOREIGN');
      backend.navigate(backend.targets[1], 'https://foreign.test/next', 'NEXT');
      backend.targets[1].frame.parentId = 'TAB';
      await held.event(
        'document-ready',
        (event) => event.frameId === 'FOREIGN' && event.loaderId === 'NEXT'
      );
      const stale = await command(
        backend.url,
        [
          'eval',
          'globalThis.marker',
          '--tab',
          'TAB',
          '--frame',
          'FOREIGN',
          '--attachment',
          ready.attachmentId,
          '--loader',
          previous.loaderId,
        ],
        home
      );
      assert.equal(stale.code, 1);
      assert.match(stale.stderr, /unavailable or changed/);
      assert.equal(
        held.events().filter((event) => event.event === 'document-ready' && event.frameId === 'TAB')
          .length,
        1
      );
      held.child.stdin.end();
      assert.equal((await held.ended).code, 0);
    } finally {
      held?.child.kill('SIGKILL');
      await backend.close();
      await rm(home, { recursive: true, force: true });
    }
  });
}

for (const route of [false, true]) {
  test(`eval frames fake: early children stay late, guarded route ${route}`, async () => {
    const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-frame-route-'));
    const backend = await evalBackend({
      intercept(message, send) {
        if (route && message.method === 'Browser.getVersion') {
          send({
            id: message.id,
            result: {
              product: 'Chrome/140.0.0.0',
              protocolVersion: '1.3',
              slicc: { route: 'link' },
            },
          });
          return true;
        }
        return false;
      },
    });
    backend.targets.push({
      targetId: 'TAB',
      url: 'https://example.test/top',
      loaderId: 'L',
      childFrames: [
        { frame: frame('SAME', 'TAB') },
        { frame: frame('CROSS', 'TAB', 'https://foreign.test') },
      ],
    });
    await writeFile(path.join(home, 'shim.js'), 'send(location.href);');
    const held = holder(
      backend.url,
      [
        'eval-attach',
        'shim.js',
        '--tab',
        'TAB',
        '--early',
        '--frames',
        'same-origin',
        '--frames',
        'https://foreign.test',
      ],
      home
    );
    try {
      await held.event('refused', (event) => event.frameId === 'TAB' && event.reason === 'late');
      assert.equal(
        (await held.event('document-ready', (event) => event.frameId === 'SAME')).early,
        false
      );
      if (route) {
        await held.event(
          'refused',
          (event) =>
            event.frameId === 'CROSS' && event.reason === 'cross-origin child tools are unavailable'
        );
        assert.ok(!backend.calls.some((call) => call.params?.uniqueContextId?.includes(':CROSS:')));
      } else {
        assert.equal(
          (await held.event('document-ready', (event) => event.frameId === 'CROSS')).early,
          false
        );
      }
      assert.ok(!held.events().some((event) => event.parentFrameId && event.early === true));
      held.child.stdin.end();
      assert.equal((await held.ended).code, 0);
    } finally {
      held.child.kill('SIGKILL');
      await backend.close();
      await rm(home, { recursive: true, force: true });
    }
  });
}

test('eval frames fake: protected children and ancestors never install and root lease survives', async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-frame-guard-'));
  const backend = await evalBackend();
  const tab = { targetId: 'TAB', url: 'https://example.test/top', loaderId: 'L' };
  backend.targets.push(tab);
  await writeFile(path.join(home, 'shim.js'), 'send(location.href);');
  const held = holder(
    backend.url,
    [
      'eval-attach',
      'shim.js',
      '--tab',
      'TAB',
      '--frames',
      'same-origin',
      '--frames',
      'https://foreign.test',
    ],
    home
  );
  try {
    await held.event('document-ready');
    tab.childFrames = [
      {
        frame: frame('PROTECTED', 'TAB', 'https://vault.sliccy.ai'),
        childFrames: [{ frame: frame('DESCENDANT', 'PROTECTED', 'https://foreign.test') }],
      },
    ];
    await held.event('refused', (event) => event.reason === 'guard');
    assert.ok(
      !backend.calls.some((call) =>
        /:(PROTECTED|DESCENDANT):/.test(call.params?.uniqueContextId ?? '')
      )
    );
    tab.childFrames = [{ frame: frame('RECOVERED', 'TAB') }];
    backend.navigate(tab, 'https://example.test/recovered', 'RECOVERY');
    await held.event('document-ready', (event) => event.frameId === 'RECOVERED');
    assert.equal(held.events().filter((event) => event.event === 'ready').length, 1);
    assert.ok(
      !held.events().some((event) => event.frameId === 'DESCENDANT' && event.event !== 'refused')
    );
    held.child.stdin.end();
    assert.equal((await held.ended).code, 0);
  } finally {
    held.child.kill('SIGKILL');
    await backend.close();
    await rm(home, { recursive: true, force: true });
  }
});

test('eval frames precheck: rejects invalid origins before connecting', async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-frame-precheck-'));
  try {
    for (const origin of [
      'https://*.example.test',
      'https://example.test/path',
      'https://user:pass@example.test',
      'file:///tmp',
      'https://example.test?query',
    ]) {
      const result = await command(
        'http://127.0.0.1:1',
        ['eval-attach', 'shim.js', '--tab', 'TAB', '--frames', origin],
        home
      );
      assert.equal(result.code, 1);
      assert.match(result.stderr, /--frames requires/);
      assert.ok(!result.stderr.includes('Connection refused'));
    }
  } finally {
    await rm(home, { recursive: true, force: true });
  }
});

for (const origin of [
  'https://seven.sliccy.ai',
  'chrome-extension://extensionid',
  'chrome-error://chromewebdata',
]) {
  test(`eval frames fake: ${origin} child and its descendant stay protected`, async () => {
    const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-frame-protected-'));
    const backend = await evalBackend();
    backend.targets.push({
      targetId: 'TAB',
      url: 'https://example.test/top',
      loaderId: 'L',
      childFrames: [
        {
          frame: frame('BLOCKED', 'TAB', origin),
          childFrames: [{ frame: frame('DESCENDANT', 'BLOCKED', 'https://foreign.test') }],
        },
      ],
    });
    await writeFile(path.join(home, 'shim.js'), 'send(location.href);');
    const held = holder(
      backend.url,
      [
        'eval-attach',
        'shim.js',
        '--tab',
        'TAB',
        '--frames',
        'same-origin',
        '--frames',
        'https://foreign.test',
      ],
      home
    );
    try {
      await Promise.race([held.event('refused'), held.event('document-ready')]);
      held.child.stdin.end();
      assert.equal((await held.ended).code, 0);
      assert.ok(
        !backend.calls.some((call) =>
          /:(BLOCKED|DESCENDANT):/.test(call.params?.uniqueContextId ?? '')
        )
      );
      assert.ok(
        !held
          .events()
          .some(
            (event) =>
              ['BLOCKED', 'DESCENDANT'].includes(event.frameId) &&
              ['document-ready', 'message'].includes(event.event)
          )
      );
    } finally {
      held.child.kill('SIGKILL');
      await backend.close();
      await rm(home, { recursive: true, force: true });
    }
  });
}

test('eval frames fake: navigation during child world creation refuses installation', async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-frame-race-'));
  const child = frame('CHILD', 'TAB');
  let raced = false;
  const backend = await evalBackend({
    intercept(message) {
      if (
        !raced &&
        message.method === 'Page.createIsolatedWorld' &&
        message.params.frameId === 'CHILD'
      ) {
        raced = true;
        child.url = 'https://miss.test/changed';
        child.securityOrigin = 'https://miss.test';
        child.loaderId = 'CHANGED';
      }
    },
  });
  backend.targets.push({
    targetId: 'TAB',
    url: 'https://example.test/top',
    loaderId: 'L',
    childFrames: [{ frame: child }],
  });
  await writeFile(path.join(home, 'shim.js'), 'send(location.href);');
  const held = holder(
    backend.url,
    ['eval-attach', 'shim.js', '--tab', 'TAB', '--world', 'isolated', '--frames', 'same-origin'],
    home
  );
  try {
    await held.event('refused', (event) => event.frameId === 'CHILD');
    assert.equal(raced, true);
    assert.ok(
      !backend.calls.some(
        (call) =>
          call.params?.expression?.startsWith('(() => { const queue = [];') &&
          call.params.uniqueContextId?.includes(':CHILD:')
      )
    );
    assert.ok(
      !held.events().some((event) => event.frameId === 'CHILD' && event.event === 'document-ready')
    );
    held.child.stdin.end();
    assert.equal((await held.ended).code, 0);
  } finally {
    held.child.kill('SIGKILL');
    await backend.close();
    await rm(home, { recursive: true, force: true });
  }
});

test('eval frames fake: requested descendant under an unrequested ordinary OOPIF ancestor', async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-frame-bridge-'));
  const backend = await evalBackend({ evalCalls: true });
  const bridge = frame('BRIDGE', 'TAB', 'https://bridge.test');
  backend.targets.push(
    { targetId: 'TAB', url: 'https://example.test/top', loaderId: 'L' },
    {
      targetId: 'BRIDGE',
      type: 'iframe',
      parentFrameId: 'TAB',
      url: bridge.url,
      loaderId: bridge.loaderId,
      frame: bridge,
      childFrames: [{ frame: frame('LEAF', 'BRIDGE', 'https://foreign.test') }],
    }
  );
  await writeFile(path.join(home, 'shim.js'), 'send(location.href);');
  const held = holder(
    backend.url,
    ['eval-attach', 'shim.js', '--tab', 'TAB', '--frames', 'https://foreign.test'],
    home
  );
  try {
    const ready = await held.event('ready');
    const leaf = await held.event('document-ready', (event) => event.frameId === 'LEAF');
    assert.deepEqual(
      leaf.ancestors.map((ancestor) => ancestor.origin),
      ['https://example.test', 'https://bridge.test']
    );
    assert.equal(leaf.parentFrameId, 'BRIDGE');
    assert.ok(
      !backend.calls.some(
        (call) =>
          call.params?.expression?.startsWith('(() => { const queue = [];') &&
          call.params.uniqueContextId?.includes(':BRIDGE:')
      )
    );
    assert.ok(
      !held.events().some((event) => event.frameId === 'BRIDGE' && event.event === 'document-ready')
    );
    const called = await command(
      backend.url,
      [
        'eval',
        'location.href',
        '--tab',
        'TAB',
        '--frame',
        'LEAF',
        '--loader',
        leaf.loaderId,
        '--attachment',
        ready.attachmentId,
      ],
      home
    );
    assert.equal(called.code, 0, called.stderr);
    assert.ok(called.stdout.includes(leaf.url));
    held.child.stdin.end();
    assert.equal((await held.ended).code, 0);
  } finally {
    held.child.kill('SIGKILL');
    await backend.close();
    await rm(home, { recursive: true, force: true });
  }
});
