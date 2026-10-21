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
const ERROR_PAGE = 'chrome-error://chromewebdata/';
const SEVEN = "slicc: refused: target is on seven's origin";
const INHERITED =
  'slicc: refused: a browser-side navigation to about: gives the page the origin of whatever drives the browser, so the guard doesn’t send it; open a new tab on about:blank instead';

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

test('a fresh playwright-cli drives an opened about:blank tab whose blocked vault error page an upstream guard masked', {
  timeout: 60_000,
}, async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-masked-'));
  const fake = await fakeCdp({ refuse: ['Page.navigate'], refusalMessage: INHERITED });
  const pw = (args) => run(['--cdp', fake.url, ...args], home);
  try {
    const opened = await pw(['open', 'about:blank']);
    assert.equal(opened.code, 0, opened.stderr);
    const [tab] = fake.targets;
    tab.url = ERROR_PAGE;
    tab.frame = { id: tab.targetId, loaderId: 'LERR', url: ERROR_PAGE, securityOrigin: '://' };

    const blank = await pw(['goto', 'about:blank']);
    assert.doesNotMatch(blank.stderr, new RegExp(SEVEN), blank.stderr);
    assert.match(blank.stderr, /a browser-side navigation to about:/, blank.stderr);
    const spaced = await pw(['goto', ' about:blank']);
    assert.doesNotMatch(spaced.stderr, new RegExp(SEVEN), spaced.stderr);
    const origin = await pw(['eval', 'self.origin']);
    assert.equal(origin.code, 0, origin.stderr);
    assert.match(origin.stdout, /fake-eval/);

    tab.url = 'http://vault.sliccy.ai/vault';
    const vault = await pw(['eval', 'self.origin']);
    assert.notEqual(vault.code, 0);
    assert.match(vault.stderr, new RegExp(SEVEN), vault.stderr);
    assert.doesNotMatch(vault.stdout, /fake-eval/);
  } finally {
    await fake.close();
    await rm(home, { recursive: true, force: true });
  }
});
