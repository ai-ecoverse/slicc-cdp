import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { fakeCdp } from './fake-cdp.mjs';
import { HARNESS_PORTS } from './ports.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const cli = path.join(
  root,
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);

function run(args, home) {
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

const childFrames = [
  {
    frame: {
      id: 'SEVEN',
      parentId: 'TAB1',
      loaderId: 'L',
      url: 'https://seven.sliccy.ai/missing',
      securityOrigin: 'https://seven.sliccy.ai',
    },
  },
  {
    frame: {
      id: 'INHERITED',
      parentId: 'TAB1',
      loaderId: 'L',
      url: 'about:blank',
      securityOrigin: 'https://vault.sliccy.ai',
    },
  },
  {
    frame: {
      id: 'PLAIN',
      parentId: 'TAB1',
      loaderId: 'L',
      url: 'http://example.test/plain',
      securityOrigin: 'http://example.test',
    },
  },
];

test('frame-scoped reads of a protected child frame committed before attach are refused', {
  timeout: 60_000,
}, async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-frames-'));
  const fake = await fakeCdp({ childFrames });
  try {
    const opened = await run(['--cdp', fake.url, 'open', 'http://example.test/page'], home);
    assert.equal(opened.code, 0, opened.stderr);
    for (const frame of ['SEVEN', 'INHERITED', 'GHOST']) {
      const shot = await run(['--cdp', fake.url, 'snapshot', `--frame=${frame}`], home);
      assert.notEqual(shot.code, 0, `${frame}: ${shot.stdout}`);
      assert.doesNotMatch(shot.stdout, /button "Go"/);
      assert.deepEqual(
        fake.calls.filter((call) => call.params?.frameId === frame).map((call) => call.method),
        [],
        frame
      );
    }
    const locked = await run(['--cdp', fake.url, 'snapshot', '--frame=PLAIN'], home);
    assert.notEqual(locked.code, 0);
    assert.match(locked.stderr, /slicc: refused:/);
    childFrames.splice(0, 2);
    const from = fake.calls.length;
    const plain = await run(['--cdp', fake.url, 'snapshot', '--frame=PLAIN'], home);
    assert.equal(plain.code, 0, plain.stderr);
    assert.deepEqual(
      fake.calls
        .slice(from)
        .filter((call) => call.params?.frameId === 'PLAIN')
        .map((call) => call.method),
      ['Page.createIsolatedWorld']
    );
  } finally {
    await fake.close();
    await rm(home, { recursive: true, force: true });
  }
});
