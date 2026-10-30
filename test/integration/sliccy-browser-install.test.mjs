import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdir, mkdtemp, readdir, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { after, test } from 'node:test';
import { nodeTransport } from '@ai-ecoverse/slicc-kernel/node';
import { launchBrowser } from './browser.mjs';
import { chromeTest } from './chrome-policy.mjs';
import { fakeCdp } from './fake-cdp.mjs';
import { kernelRunner } from './runners.mjs';
import { startServer } from './server.mjs';
import './watchdog.mjs';

const PACKAGE = path.resolve('packages/sliccy-browser');
const NAME = '@ai-ecoverse/sliccy-browser';
const PUBLISHED = '0.1.34';
const COMMANDS = ['playwright-cli', 'curlwright'];
const BASE = COMMANDS.map((name) => `/usr/bin/${name}`);
const ROOTS = '$PNPM_HOME/global/v11/[0-9]-*/node_modules';
const TARBALL = 'file:/srv/sliccy-browser.tgz';
const wasm = (name) => path.resolve('target/wasm32-wasip1/debug', `${name}.wasm`);

const scratch = await mkdtemp(path.join(tmpdir(), 'sliccy-browser-install-'));
after(() => rm(scratch, { recursive: true, force: true }));

async function pack(spec, name) {
  const dir = path.join(scratch, name);
  await mkdir(dir);
  const [{ filename }] = JSON.parse(
    execFileSync('npm', ['pack', spec, '--pack-destination', dir, '--json', '--silent'], {
      encoding: 'utf8',
    })
  );
  const tarball = path.join(dir, filename);
  execFileSync('tar', ['xzf', tarball, '-C', dir]);
  return { tarball, dir };
}

const tree = await pack(PACKAGE, 'tree');

async function shipped({ dir }) {
  const src = path.join(dir, 'package/src');
  const files = {};
  for (const file of await readdir(src)) files[file] = await readFile(path.join(src, file), 'utf8');
  const manifest = JSON.parse(await readFile(path.join(dir, 'package/package.json'), 'utf8'));
  return { files, dependencies: manifest.dependencies ?? {} };
}

async function cone(t, wsUrl) {
  const kernel = await kernelRunner(wasm('playwright-cli'), wasm('curlwright'), wsUrl, {
    network: { transport: nodeTransport() },
  });
  t.after(() => kernel.close());
  for (const name of ['wasm-bash', 'wasm-coreutils', 'wasm-tls-engine', 'wasi-pnpm', 'slicc-jsh'])
    await kernel.install(`@ai-ecoverse/${name}`);
  await kernel.addUser('cone');
  await kernel.writeFile('/srv/sliccy-browser.tgz', await readFile(tree.tarball));
  return {
    kernel,
    sh: (script) => kernel.run(['bash', '-c', script], '/', { user: 'cone' }),
  };
}

async function added({ sh }, spec) {
  const run = await sh(`pnpm add -g --config.minimum-release-age=0 ${spec}`);
  assert.equal(run.code, 0, `${run.stdout}${run.stderr}`);
}

async function found({ sh }) {
  const run = await sh('command -v playwright-cli curlwright; playwright-cli --version');
  assert.equal(run.code, 0, run.stderr);
  const lines = run.stdout.trim().split('\n');
  return { commands: lines.slice(0, 2), version: lines[2] };
}

async function provided({ sh }) {
  const run = await sh(
    `for f in ${ROOTS}/*/package.json ${ROOTS}/@*/*/package.json; do cat "$f"; printf '\\0'; done`
  );
  assert.equal(run.code, 0, run.stderr);
  return run.stdout
    .split('\0')
    .filter(Boolean)
    .map((text) => JSON.parse(text))
    .flatMap(({ name, bin, slicc }) =>
      [
        ...(typeof bin === 'string' ? [name.split('/').pop()] : Object.keys(bin ?? {})),
        ...Object.keys(slicc?.commands ?? {}),
      ]
        .filter((command) => COMMANDS.includes(command))
        .map((command) => `${name} provides ${command}`)
    );
}

async function work({ kernel, sh }, url, steps) {
  await kernel.writeFile(
    '/srv/work.jsh',
    `const browser = require('sliccy:browser');
browser.ensureTab(process.argv[2]).then(async (tab) => {
  console.log(JSON.stringify({ ${steps} }));
  process.exit(0);
});
`
  );
  const run = await sh(`jsh /srv/work.jsh ${url}`);
  assert.equal(run.code, 0, `${run.stdout}${run.stderr}`);
  return JSON.parse(run.stdout);
}

const FAKE_STEPS = `
  found: (await browser.findTab({ domain: 'example.test' }))?.targetId,
  value: await browser.eval(tab, 'document.title'),
  tab: tab.targetId,
`;
const FAKE_WORK = { found: 'TAB1', value: 'fake-eval', tab: 'TAB1' };

test('sliccy-browser’s copies of the CDP client are the same bytes as src/', async () => {
  for (const file of ['connect.js', 'cdp-client.js'])
    assert.equal(
      await readFile(path.join(PACKAGE, 'src', file), 'utf8'),
      await readFile(path.resolve('src', file), 'utf8'),
      `run: cp src/${file} packages/sliccy-browser/src/, and raise its version`
    );
});

test('what sliccy-browser ships is what npm has under its version, so a change to it names a new version for ci.yml to publish', async (t) => {
  const { version } = JSON.parse(await readFile(path.join(PACKAGE, 'package.json'), 'utf8'));
  const spec = `${NAME}@${version}`;
  if (spawnSync('npm', ['view', spec, 'version'], { stdio: 'ignore' }).status !== 0) {
    t.diagnostic(`${spec} isn't on npm yet, so ci.yml publishes it`);
    return;
  }
  assert.deepEqual(
    await shipped(tree),
    await shipped(await pack(spec, 'npm')),
    `raise sliccy-browser's version: ${spec} on npm isn't this tree`
  );
});

test('on a real kernel and pnpm, a fresh pnpm add -g leaves a user the base set’s playwright-cli and curlwright, nothing it installs provides either, and sliccy:browser works', async (t) => {
  const fake = await fakeCdp();
  t.after(() => fake.close());
  const user = await cone(t, fake.url);
  await added(user, TARBALL);
  assert.deepEqual(await provided(user), []);
  assert.deepEqual((await found(user)).commands, BASE);
  assert.deepEqual(await work(user, 'https://example.test/', FAKE_STEPS), FAKE_WORK);
});

test(`on a real kernel and pnpm, updating from the published ${PUBLISHED}, whose closure provides both commands, leaves none that does and the user the base set’s, and sliccy:browser works`, async (t) => {
  const fake = await fakeCdp();
  t.after(() => fake.close());
  const user = await cone(t, fake.url);
  await added(user, `${NAME}@${PUBLISHED}`);
  assert.deepEqual(
    await provided(user),
    COMMANDS.map((command) => `@ai-ecoverse/slicc-cdp provides ${command}`)
  );
  await added(user, TARBALL);
  assert.deepEqual(await provided(user), []);
  const after = await found(user);
  assert.deepEqual(after.commands, BASE);
  assert.notEqual(after.version, 'playwright-cli 1.30.18');
  assert.deepEqual(await work(user, 'https://example.test/', FAKE_STEPS), FAKE_WORK);
});

chromeTest(
  'in a kernel’s jsh, the installed package opens, finds, reads and fetches in a real tab',
  async (t) => {
    const site = await startServer();
    const chrome = await launchBrowser();
    t.after(async () => {
      await chrome.close();
      await site.close();
    });
    const user = await cone(t, chrome.wsUrl);
    await added(user, TARBALL);
    const steps = `
    url: tab.url,
    title: tab.title,
    found: (await browser.findTab({ urlMatch: '/page$' }))?.targetId === tab.targetId,
    evaluated: await browser.eval(tab, () => document.title),
    sum: await browser.evalAsync(tab, async () => 2 + 2),
    cookie: await browser.cookie(tab, 'session'),
    stored: await browser.eval(tab, () => localStorage.setItem('n', '123')).then(() => browser.localStorage(tab, 'n')),
    fetched: await browser.fetch(tab, ${JSON.stringify(`${site.origin}/api/me`)}).then(({ status, body }) => ({ status, body })),
  `;
    assert.deepEqual(await work(user, `${site.origin}/page`, steps), {
      url: `${site.origin}/page`,
      title: 'Fixture',
      found: true,
      evaluated: 'Fixture',
      sum: 4,
      cookie: 'from-tab',
      stored: '123',
      fetched: { status: 200, body: 'session=from-tab' },
    });
  }
);
