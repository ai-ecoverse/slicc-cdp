import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
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
const fixtures = path.join(root, 'test/integration/fixtures/snapshot');
const read = (name) => readFileSync(path.join(fixtures, name), 'utf8');
const titles = { example: 'Example Domain', forms: 'Sign up', links: 'Links', frames: 'Frames' };
const pageUrl = (name) => `http://127.0.0.1:9/snapshot/${name}.html`;

function refsOf(nodes, out = []) {
  for (const node of nodes) {
    if (typeof node === 'string') continue;
    if (node.ref) out.push(node.ref);
    refsOf(node.children ?? [], out);
  }
  return out;
}

function capture(json, base, frames = {}) {
  const refs = refsOf(json);
  return {
    json,
    base,
    refs: refs.map((ref) => ({ ref, selector: `:root > [data-ref="${ref}"]` })),
    nodes: refs.map((ref, index) => ({
      backendNodeId: 100 + index,
      ...(frames[ref] ? { frameId: frames[ref] } : {}),
    })),
  };
}

function page(name) {
  const json = JSON.parse(read(`${name}.upstream.json`));
  const base = pageUrl(name);
  if (name !== 'frames') return { aria: () => capture(json, base) };
  const inner = [];
  const outer = structuredClone(json);
  const strip = (nodes) => {
    for (const node of nodes) {
      if (typeof node === 'string') continue;
      if (node.role === 'iframe') {
        inner.push(...(node.children ?? []));
        delete node.children;
      }
      strip(node.children ?? []);
    }
  };
  strip(outer);
  return {
    aria: (options) =>
      options.prefix === 'f1'
        ? capture(inner, 'http://127.0.0.1:9/snapshot/frame-child.html')
        : capture(outer, base, { e3: 'CHILD' }),
    frameTree: {
      frame: { id: 'TAB1', loaderId: 'L', url: base },
      childFrames: [{ frame: { id: 'CHILD', loaderId: 'L', url: `${base}#child` } }],
    },
  };
}

function expected(name, yaml) {
  return `### Page\n- Page URL: ${pageUrl(name)}\n- Page Title: ${titles[name]}\n### Snapshot\n\`\`\`yaml\n${yaml}\`\`\`\n`;
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
    const child = spawn(cli, args, { cwd: home, env, stdio: ['ignore', 'pipe', 'pipe'] });
    const out = [];
    const err = [];
    child.stdout.on('data', (chunk) => out.push(chunk));
    child.stderr.on('data', (chunk) => err.push(chunk));
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      reject(new Error(`timed out: ${args.join(' ')}`));
    }, 30000);
    child.on('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on('close', (code) => {
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
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-snapshot-'));
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

async function drive(start, name, body) {
  const fake = await fakeCdp({
    ...page(name),
    location: { href: pageUrl(name), title: titles[name] },
  });
  const runner = await start(fake.url);
  try {
    const run = async (args) => {
      const result = await runner.run(fake.url, args);
      assert.equal(result.code, 0, `${args.join(' ')}: ${result.stderr}`);
      return result;
    };
    await run(['open', pageUrl(name)]);
    await body(run, fake);
  } finally {
    await runner.close();
    await fake.close();
  }
}

test('the committed snapshots differ from upstream only where a /url: is redacted', () => {
  for (const name of Object.keys(titles)) {
    const ours = read(`${name}.yml`).split('\n');
    const upstream = read(`${name}.upstream.yml`).split('\n');
    assert.equal(ours.length, upstream.length, name);
    for (const [index, line] of ours.entries()) {
      if (line !== upstream[index]) assert.match(line, /^ *- \/url: /, `${name}:${index + 1}`);
    }
  }
});

for (const [label, start] of [
  ['native', nativeRunner],
  ['wasi', wasiRunner],
]) {
  test(`${label} snapshot prints upstream's tree for the fixture pages`, async () => {
    assert.ok(label === 'native' || existsSync(path.join(wasmDir, 'playwright-cli.wasm')));
    for (const name of Object.keys(titles)) {
      await drive(start, name, async (run, fake) => {
        const shot = await run(['snapshot']);
        assert.equal(shot.stdout, expected(name, read(`${name}.yml`)), name);
        assert.deepEqual(
          fake.calls.filter((call) => call.method === 'Accessibility.getFullAXTree'),
          []
        );
      });
    }
  });

  test(`${label} snapshot of an example.com page is no larger than upstream's`, async () => {
    await drive(start, 'example', async (run) => {
      const shot = await run(['snapshot']);
      const upstream = expected('example', read('example.upstream.yml'));
      assert.ok(
        Buffer.byteLength(shot.stdout) <= Buffer.byteLength(upstream) * 1.02,
        `${Buffer.byteLength(shot.stdout)} > ${Buffer.byteLength(upstream)}`
      );
      assert.doesNotMatch(shot.stdout, /inlinetextbox|statictext|rootwebarea/);
    });
  });

  test(`${label} snapshot redacts a protected link's /url:`, async () => {
    await drive(start, 'links', async (run) => {
      const shot = await run(['snapshot']);
      assert.match(
        shot.stdout,
        /link "Seven" \[ref=e5\] \[cursor=pointer\]:\n {8}- \/url: https:\/\/seven\.sliccy\.ai\n/
      );
      assert.match(
        shot.stdout,
        /link "Vault" \[ref=e7\] \[cursor=pointer\]:\n {8}- \/url: http:\/\/vault\.sliccy\.ai\n/
      );
      assert.doesNotMatch(shot.stdout, /SECRET|\/os\/|\/v\/item|inbox|user:pass/);
      assert.match(shot.stdout, /- \/url: \/next\?page=…\n/);
    });
  });

  test(`${label} snapshot refs drive click in the page and in an iframe`, async () => {
    await drive(start, 'example', async (run, fake) => {
      await run(['snapshot']);
      const from = fake.calls.length;
      await run(['click', 'e13']);
      const resolved = fake.calls.slice(from).filter((call) => call.method === 'DOM.resolveNode');
      assert.deepEqual(
        resolved.map((call) => call.params.backendNodeId),
        [100 + refsOf(JSON.parse(read('example.upstream.json'))).indexOf('e13')]
      );
    });
    await drive(start, 'frames', async (run, fake) => {
      await run(['snapshot']);
      const from = fake.calls.length;
      await run(['click', 'f1e3']);
      const clicked = fake.calls
        .slice(from)
        .filter(
          (call) =>
            call.method === 'Runtime.evaluate' &&
            call.params.expression?.includes('data-ref=\\"f1e3\\"')
        );
      assert.equal(
        clicked.length,
        1,
        JSON.stringify(fake.calls.slice(from).map((call) => call.method))
      );
      const world = fake.calls
        .slice(from)
        .find((call) => call.method === 'Page.createIsolatedWorld');
      assert.equal(world?.params.frameId, 'CHILD');
    });
  });
}
