import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { fakeCdp } from './fake-cdp.mjs';
import { HARNESS_PORTS } from './ports.mjs';
import { kernelRunner } from './runners.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const cli = path.join(
  root,
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);
const wasmDir = path.join(root, 'target/wasm32-wasip1/debug');
const base = 'http://127.0.0.1:8765/fr.html';
const child = 'http://localhost:8765/child.html';

function refsOf(nodes, out = []) {
  for (const node of nodes) {
    if (typeof node === 'string') continue;
    if (node.ref) out.push(node.ref);
    refsOf(node.children ?? [], out);
  }
  return out;
}

function capture(json, url, frames = {}) {
  const refs = refsOf(json);
  return {
    json,
    base: url,
    refs: refs.map((ref) => ({ ref, selector: `:root > [data-ref="${ref}"]` })),
    nodes: refs.map((ref, index) => ({
      backendNodeId: 100 + index,
      ...(frames[ref] ? { frameId: frames[ref] } : {}),
    })),
  };
}

function remote(targetId, url, securityOrigin = url ? new URL(url).origin : '') {
  return {
    targetId,
    type: 'iframe',
    url,
    parentFrameId: 'TAB1',
    tree: { frame: { id: targetId, parentId: 'TAB1', loaderId: 'L', url, securityOrigin } },
  };
}

function aria(iframes) {
  return (options, _params, target) => {
    if (target?.type === 'iframe') {
      const name = target.targetId === 'XO' ? 'Inner' : `${target.targetId}_CONTENT`;
      return capture(
        [
          {
            role: 'generic',
            ref: `${options.prefix}e1`,
            children: [{ role: 'button', name, ref: `${options.prefix}e2` }],
          },
        ],
        target.url
      );
    }
    const frames = Object.fromEntries(iframes.map((id, index) => [`e${index + 3}`, id]));
    return capture(
      [
        {
          role: 'generic',
          active: true,
          ref: 'e1',
          children: [
            { role: 'heading', name: 'Top', level: 1, ref: 'e2' },
            ...iframes.map((_, index) => ({ role: 'iframe', ref: `e${index + 3}` })),
          ],
        },
      ],
      base,
      frames
    );
  };
}

function native(args, home) {
  return new Promise((resolve, reject) => {
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
    delete env.PLAYWRIGHT_CLI_SESSION;
    const spawned = spawn(cli, args, { cwd: home, env, stdio: ['ignore', 'pipe', 'pipe'] });
    const out = [];
    const err = [];
    spawned.stdout.on('data', (chunk) => out.push(chunk));
    spawned.stderr.on('data', (chunk) => err.push(chunk));
    const timer = setTimeout(() => {
      spawned.kill('SIGKILL');
      reject(new Error(`timed out: ${args.join(' ')}`));
    }, 30000);
    spawned.on('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
    spawned.on('close', (code) => {
      clearTimeout(timer);
      resolve({
        code: code ?? 1,
        stdout: Buffer.concat(out).toString('utf8'),
        stderr: Buffer.concat(err).toString('utf8'),
      });
    });
  });
}

async function nativeRunner() {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-oopif-'));
  return {
    run: (url, args) => native(['--cdp', url, ...args], home),
    close: () => rm(home, { recursive: true, force: true }),
  };
}

async function wasiRunner(url) {
  const kernel = await kernelRunner(
    path.join(wasmDir, 'playwright-cli.wasm'),
    path.join(wasmDir, 'curlwright.wasm'),
    url
  );
  const cwd = await kernel.fresh();
  return {
    run: (_url, args) => kernel.playwright(args, cwd),
    close: () => kernel.close(),
  };
}

async function drive(start, iframes, targets, body) {
  const fake = await fakeCdp({ aria: aria(iframes), location: { href: base, title: 'Frames' } });
  const runner = await start(fake.url);
  try {
    const attempt = (args) => runner.run(fake.url, args);
    const run = async (args) => {
      const result = await attempt(args);
      assert.equal(result.code, 0, `${args.join(' ')}: ${result.stderr}`);
      return result;
    };
    await run(['open', base]);
    fake.targets.push(...targets);
    await body(run, fake, attempt);
  } finally {
    await runner.close();
    await fake.close();
  }
}

function shot(lines) {
  return `### Page\n- Page URL: ${base}\n- Page Title: Frames\n### Snapshot\n\`\`\`yaml\n${lines.join('\n')}\n\`\`\`\n`;
}

const inner = (frame, owner) => [
  `  - iframe [ref=${owner}]:`,
  `    - generic [ref=${frame}e1]:`,
  `      - button "Inner" [ref=${frame}e2]`,
];

const head = ['- generic [active] [ref=e1]:', '  - heading "Top" [level=1] [ref=e2]'];

const bound = (fake, target) => fake.calls.filter((call) => call.target === target);

for (const [label, start] of [
  ['native', nativeRunner],
  ['wasi', wasiRunner],
]) {
  test(`${label} snapshot and frames include a cross-origin iframe whose refs act in its own session`, async () => {
    assert.ok(label === 'native' || existsSync(path.join(wasmDir, 'playwright-cli.wasm')));
    await drive(start, ['XO'], [remote('XO', child)], async (run, fake) => {
      const snapshot = await run(['snapshot']);
      assert.equal(snapshot.stdout, shot([...head, ...inner('f1', 'e3')]));
      assert.ok(
        fake.calls.some(
          (call) =>
            call.method === 'Target.attachToTarget' &&
            call.params.targetId === 'XO' &&
            call.params.flatten === true &&
            !call.sessionId
        )
      );
      assert.ok(
        bound(fake, 'XO').some(
          (call) => call.method === 'Page.createIsolatedWorld' && call.params.frameId === 'XO'
        )
      );
      const frames = await run(['frames']);
      assert.match(frames.stdout, /\[main\] frameId=TAB1 - http:\/\/127\.0\.0\.1:8765\/fr\.html\n/);
      assert.match(
        frames.stdout,
        /\[child\] frameId=XO parentFrameId=TAB1 - http:\/\/localhost:8765\/child\.html\n/
      );
      const from = fake.calls.length;
      const clicked = await run(['click', 'f1e2']);
      assert.equal(clicked.stdout, 'Clicked f1e2\n');
      const evaluated = fake.calls
        .slice(from)
        .filter(
          (call) => call.method === 'Runtime.evaluate' && call.params.expression?.includes('f1e2')
        );
      assert.deepEqual(
        evaluated.map((call) => call.target),
        ['XO']
      );
      await run(['snapshot']);
      const hovered = fake.calls.length;
      await run(['hover', 'f1e2']);
      const moves = fake.calls
        .slice(hovered)
        .filter((call) => call.method === 'Input.dispatchMouseEvent');
      assert.deepEqual(
        moves.map((call) => [call.target, call.params.type, call.params.x, call.params.y]),
        [['TAB1', 'mouseMoved', 5, 5]]
      );
      assert.ok(
        fake.calls
          .slice(hovered)
          .some(
            (call) =>
              call.method === 'DOM.resolveNode' &&
              call.target === 'TAB1' &&
              call.params.backendNodeId === 102
          )
      );
      const scoped = fake.calls.length;
      const value = await run(['eval', '--frame=XO', 'document.title']);
      assert.equal(value.stdout, 'fake-eval\n');
      assert.ok(
        fake.calls
          .slice(scoped)
          .some(
            (call) =>
              call.method === 'Runtime.evaluate' &&
              call.target === 'XO' &&
              call.params.expression === 'document.title'
          )
      );
      const chosen = await run(['snapshot', '--frame=XO']);
      assert.match(
        chosen.stdout,
        /```yaml\n- generic \[ref=f1e1\]:\n {2}- button "Inner" \[ref=f1e2\]\n```/
      );
    });
  });

  test(`${label} snapshot shows a protected cross-origin iframe as a bare placeholder and never reads it`, async () => {
    const seven = remote('SEVEN', 'https://seven.sliccy.ai/os/?k=SECRET#inbox');
    await drive(
      start,
      ['SEVEN', 'XO'],
      [seven, remote('XO', child)],
      async (run, fake, attempt) => {
        const snapshot = await run(['snapshot']);
        assert.equal(snapshot.stdout, shot([...head, '  - iframe [ref=e3]', ...inner('f2', 'e4')]));
        const frames = await run(['frames']);
        assert.match(
          frames.stdout,
          /\[child\] frameId=SEVEN parentFrameId=TAB1 - https:\/\/seven\.sliccy\.ai\n/
        );
        assert.match(frames.stdout, /\[child\] frameId=XO parentFrameId=TAB1 - /);
        const chosen = await attempt(['snapshot', '--frame=SEVEN']);
        assert.notEqual(chosen.code, 0);
        const evaluated = await attempt(['eval', '--frame=SEVEN', 'document.body.innerText']);
        assert.notEqual(evaluated.code, 0);
        for (const output of [snapshot, frames, chosen, evaluated]) {
          assert.doesNotMatch(output.stdout + output.stderr, /SECRET|SEVEN_CONTENT|\/os\/|inbox/);
        }
        assert.deepEqual(bound(fake, 'SEVEN'), []);
        assert.deepEqual(
          fake.calls.filter(
            (call) => call.method === 'Target.attachToTarget' && call.params.targetId === 'SEVEN'
          ),
          []
        );
      }
    );
  });

  test(`${label} snapshot fails closed on an unknown or unverified cross-origin iframe`, async () => {
    const unverified = remote('UNVERIFIED', 'http://localhost:8765/unverified.html', '');
    await drive(
      start,
      ['GHOST', 'BLANK', 'UNVERIFIED', 'XO'],
      [remote('BLANK', ''), unverified, remote('XO', child)],
      async (run, fake) => {
        const snapshot = await run(['snapshot']);
        assert.equal(
          snapshot.stdout,
          shot([
            ...head,
            '  - iframe [ref=e3]',
            '  - iframe [ref=e4]',
            '  - iframe [ref=e5]',
            ...inner('f3', 'e6'),
          ])
        );
        assert.doesNotMatch(snapshot.stdout, /_CONTENT/);
        assert.deepEqual(
          fake.calls.filter((call) => call.params?.frameId === 'GHOST'),
          []
        );
        assert.deepEqual(bound(fake, 'BLANK'), []);
        assert.deepEqual(
          bound(fake, 'UNVERIFIED').filter((call) =>
            ['Page.createIsolatedWorld', 'Runtime.evaluate', 'DOM.describeNode'].includes(
              call.method
            )
          ),
          []
        );
      }
    );
  });
}
