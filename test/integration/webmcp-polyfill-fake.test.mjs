import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { kernelRunner } from './runners.mjs';
import { webmcpBackend } from './webmcp-backend.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const binary = path.join(
  root,
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);

function launch(args, cwd, endpoint) {
  const env = { ...process.env, HOME: cwd, USERPROFILE: cwd, APPDATA: cwd, XDG_CONFIG_HOME: cwd };
  delete env.SLICC_CDP_URL;
  const child = spawn(binary, ['--cdp', endpoint, ...args], {
    cwd,
    env,
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  let stdout = '';
  let stderr = '';
  child.stdout.on('data', (bytes) => {
    stdout += bytes;
  });
  child.stderr.on('data', (bytes) => {
    stderr += bytes;
  });
  const ended = new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('close', (code) => resolve({ code, stdout, stderr }));
  });
  const ready = new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`holder status did not arrive: ${stderr}`)),
      15000
    );
    const onData = () => {
      const line = stdout.split('\n')[0];
      if (!stdout.includes('\n')) return;
      clearTimeout(timer);
      child.stdout.off('data', onData);
      try {
        resolve(JSON.parse(line));
      } catch (error) {
        reject(error);
      }
    };
    child.stdout.on('data', onData);
    ended.then((result) => {
      clearTimeout(timer);
      if (!stdout.includes('\n')) reject(new Error(result.stderr));
    }, reject);
  });
  ready.catch(() => {});
  return { child, ended, ready };
}

async function run(args, cwd, endpoint) {
  const process = launch(args, cwd, endpoint);
  process.child.stdin.end();
  return process.ended;
}

async function fixture(policy, check, children = []) {
  const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-webmcp-fake-'));
  const backend = await webmcpBackend({
    policy,
    children,
    definitions: [
      {
        name: 'greet',
        description: 'Greets',
        inputSchema: { type: 'object' },
        annotations: { readOnly: true },
        execute: (input, client) => {
          assert.ok(client.signal);
          return { greeting: input.name };
        },
      },
      {
        name: 'risky',
        description: 'Consequential',
        annotations: { consequential: true },
        execute: () => 'executed',
      },
      { name: 'large', execute: () => 'x'.repeat(100000) },
    ],
  });
  let holder;
  try {
    holder = launch(['webmcp-polyfill', '--tab', 'TAB', '--reload'], cwd, backend.url);
    let status;
    try {
      status = await holder.ready;
    } catch (error) {
      error.message += `\nCDP tail: ${backend.calls
        .slice(-50)
        .map((call) => `${call.method}:${call.sessionId || ''}`)
        .join(', ')}`;
      throw error;
    }
    assert.equal(status.mode, 'polyfill');
    assert.equal(status.reloaded, true);
    assert.equal(JSON.stringify(status).includes('__wmcp_'), false);
    await check(backend, cwd, status);
    holder.child.stdin.end();
    assert.equal((await holder.ended).code, 0);
    assert.ok(
      backend.calls.some((call) => call.method === 'Page.removeScriptToEvaluateOnNewDocument')
    );
    assert.ok(backend.calls.every((call) => call.method !== 'Runtime.addBinding'));
    assert.equal(backend.scripts.size, 0);
  } finally {
    holder?.child.kill('SIGKILL');
    await backend.close();
    await rm(cwd, { recursive: true, force: true });
  }
}

await test('fake: holder reload, private bridge listing/calling, consequential gate and output cap', async () => {
  await fixture('', async (backend, cwd, status) => {
    assert.ok(status.pauseMicros.length >= 2);
    const listed = await run(['webmcp-list', '--tab', 'TAB', '--json'], cwd, backend.url);
    assert.equal(listed.code, 0, listed.stderr);
    const tools = JSON.parse(listed.stdout);
    assert.deepEqual(
      tools.map((tool) => tool.name),
      ['greet', 'large', 'risky']
    );
    assert.ok(tools.every((tool) => tool.polyfilled && tool.frameId === 'TAB'));
    const called = await run(
      ['webmcp-call', 'greet', '{"name":"Alice"}', '--tab', 'TAB'],
      cwd,
      backend.url
    );
    assert.equal(called.code, 0, called.stderr);
    assert.deepEqual(JSON.parse(called.stdout), { greeting: 'Alice' });
    const risky = await run(['webmcp-call', 'risky', '{}', '--tab', 'TAB'], cwd, backend.url);
    assert.equal(risky.code, 2);
    const allowed = await run(
      ['webmcp-call', 'risky', '{}', '--tab', 'TAB', '--allow-consequential'],
      cwd,
      backend.url
    );
    assert.equal(allowed.code, 0, allowed.stderr);
    assert.equal(allowed.stdout.trim(), 'executed');
    const large = await run(['webmcp-call', 'large', '{}', '--tab', 'TAB'], cwd, backend.url);
    assert.equal(large.code, 0, large.stderr);
    assert.equal(large.stdout.trim().length, 65536);
  });
});

await test('fake: same-origin aggregation and explicit foreign exposure/delegation with ambiguous tool selection', async () => {
  await fixture(
    '',
    async (backend, cwd) => {
      const list = async (extra = []) => {
        const result = await run(
          ['webmcp-list', '--tab', 'TAB', '--json', ...extra],
          cwd,
          backend.url
        );
        assert.equal(result.code, 0, result.stderr);
        return JSON.parse(result.stdout);
      };
      const ordinary = await list();
      assert.equal(ordinary.length, 4);
      assert.equal(ordinary.filter((tool) => tool.name === 'greet').length, 2);
      const cross = await list([
        '--from-origin',
        'https://foreign.test',
        '--from-origin',
        'https://foreign.test/',
      ]);
      assert.equal(cross.length, 5);
      assert.equal(cross.find((tool) => tool.name === 'foreign').origin, 'https://foreign.test');
      assert.equal(
        cross.some((tool) => tool.name === 'hidden'),
        false
      );
      const ambiguous = await run(['webmcp-call', 'greet', '{}', '--tab', 'TAB'], cwd, backend.url);
      assert.notEqual(ambiguous.code, 0);
      assert.match(ambiguous.stderr, /ambiguous/);
      const selected = await run(
        ['webmcp-call', 'greet', '{}', '--tab', 'TAB', '--frame', 'SAME'],
        cwd,
        backend.url
      );
      assert.equal(selected.code, 0, selected.stderr);
      assert.equal(selected.stdout.trim(), 'same-origin child');
      const denied = await run(['webmcp-call', 'foreign', '{}', '--tab', 'TAB'], cwd, backend.url);
      assert.equal(denied.code, 2);
      const called = await run(
        ['webmcp-call', 'foreign', '{}', '--tab', 'TAB', '--from-origin', 'https://foreign.test'],
        cwd,
        backend.url
      );
      assert.equal(called.code, 0, called.stderr);
      assert.equal(called.stdout.trim(), 'foreign child');
    },
    [
      {
        id: 'SAME',
        origin: 'https://ordinary.test',
        definitions: [{ name: 'greet', execute: () => 'same-origin child' }],
      },
      {
        id: 'CROSS',
        origin: 'https://foreign.test',
        allow: 'tools *',
        definitions: [
          { name: 'foreign', exposedTo: ['https://ordinary.test'], execute: () => 'foreign child' },
          {
            name: 'hidden',
            exposedTo: ['https://other.test'],
            execute: () => assert.fail('unexposed tool invoked'),
          },
        ],
      },
    ]
  );
});

await test('fake: parent policy, child policy and absent iframe delegation each deny exposed foreign tools', async () => {
  for (const [parentPolicy, childPolicy, allow] of [
    ['tools=(self)', '', 'tools *'],
    ['', 'tools=()', 'tools *'],
    ['', '', ''],
  ]) {
    await fixture(
      parentPolicy,
      async (backend, cwd) => {
        const listed = await run(
          ['webmcp-list', '--tab', 'TAB', '--json', '--from-origin', 'https://foreign.test'],
          cwd,
          backend.url
        );
        assert.equal(listed.code, 0, listed.stderr);
        assert.equal(
          JSON.parse(listed.stdout).some((tool) => tool.name === 'foreign'),
          false
        );
        const called = await run(
          ['webmcp-call', 'foreign', '{}', '--tab', 'TAB', '--from-origin', 'https://foreign.test'],
          cwd,
          backend.url
        );
        assert.equal(called.code, 2);
      },
      [
        {
          id: 'CROSS',
          origin: 'https://foreign.test',
          policy: childPolicy,
          allow,
          definitions: [
            {
              name: 'foreign',
              exposedTo: ['https://ordinary.test'],
              execute: () => assert.fail('policy-denied tool invoked'),
            },
          ],
        },
      ]
    );
  }
});

await test('fake: native prototype does not make an absent or null page API available', async () => {
  for (const api of [undefined, null, { getTools: async () => [] }]) {
    const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-webmcp-availability-'));
    const backend = await webmcpBackend({ native: true });
    try {
      Object.defineProperty(backend.page.document, 'modelContext', { value: api });
      for (const args of [
        ['webmcp-list', '--json'],
        ['webmcp-call', 'anything', '{}'],
      ]) {
        const result = await run([...args, '--tab', 'TAB'], cwd, backend.url);
        if (api == null) {
          assert.equal(result.code, 2, result.stderr);
          assert.equal(result.stdout, '');
          assert.match(result.stderr, /WebMCP is unavailable/);
        } else if (args[0] === 'webmcp-list') {
          assert.equal(result.code, 0, result.stderr);
          assert.equal(result.stdout, '[]\n');
        } else {
          assert.match(result.stderr, /unknown tool/);
        }
      }
    } finally {
      await backend.close();
      await rm(cwd, { recursive: true, force: true });
    }
  }
});

await test('fake: native child calls recheck loader, policy and delegation and preserve protected refusals', async () => {
  for (const mode of ['stable', 'loader', 'policy', 'delegation', 'protected', 'during']) {
    const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-webmcp-native-'));
    let invoked = 0;
    let backend;
    const children = [
      {
        id: 'CROSS',
        origin: 'https://cross.test',
        allow: 'tools *',
        definitions: [
          {
            name: 'child',
            annotations: { readOnly: true },
            execute: () => {
              invoked++;
              if (mode === 'during') backend.pages.get('CROSS').loader = 'NAVIGATED';
              return { invoked };
            },
          },
        ],
      },
    ];
    try {
      backend = await webmcpBackend({
        native: true,
        children,
        onCommand(command, pages) {
          if (command.method !== 'Browser.getVersion') return;
          if (mode === 'loader') pages.get('CROSS').loader = 'NAVIGATED';
          if (mode === 'policy') pages.get('CROSS').permitted = false;
          if (mode === 'delegation') children[0].allow = "tools 'none'";
          if (mode === 'protected') pages.get('CROSS').origin = 'https://seven.sliccy.ai';
        },
      });
      const called = await run(
        ['webmcp-call', 'child', '{}', '--tab', 'TAB', '--from-origin', 'https://cross.test'],
        cwd,
        backend.url
      );
      if (mode === 'stable' || mode === 'during') {
        assert.equal(called.code, 0, called.stderr);
        assert.deepEqual(JSON.parse(called.stdout), mode === 'during' ? null : { invoked: 1 });
        assert.equal(invoked, 1);
      } else {
        assert.notEqual(called.code, 0, mode);
        assert.equal(called.stdout, '', mode);
        assert.equal(invoked, 0, mode);
        if (mode === 'protected') assert.match(called.stderr, /slicc: refused:/);
      }
    } finally {
      await backend?.close();
      await rm(cwd, { recursive: true, force: true });
    }
  }
});

await test('fake: WASI holder accepts EOF and removes future injection without a binding', async () => {
  const backend = await webmcpBackend();
  let runner;
  try {
    runner = await kernelRunner(
      path.join(root, 'target/wasm32-wasip1/debug/playwright-cli.wasm'),
      path.join(root, 'target/wasm32-wasip1/debug/curlwright.wasm'),
      backend.url
    );
    const cwd = await runner.fresh();
    const held = await runner.playwright(['webmcp-polyfill', '--tab', 'TAB', '--reload'], cwd, {
      stdin: '',
    });
    assert.equal(held.code, 0, held.stderr);
    const status = JSON.parse(held.stdout.trim());
    assert.equal(status.mode, 'polyfill');
    assert.equal(backend.scripts.size, 0);
    assert.ok(backend.calls.every((call) => call.method !== 'Runtime.addBinding'));
  } finally {
    await runner?.close();
    await backend.close();
  }
});

await test('fake: tools Permissions Policy denies bridge listing and invocation', async () => {
  await fixture('tools=()', async (backend, cwd) => {
    const listed = await run(['webmcp-list', '--tab', 'TAB', '--json'], cwd, backend.url);
    assert.equal(listed.code, 0, listed.stderr);
    assert.deepEqual(JSON.parse(listed.stdout), []);
    const called = await run(['webmcp-call', 'greet', '{}', '--tab', 'TAB'], cwd, backend.url);
    assert.equal(called.code, 2);
  });
});
