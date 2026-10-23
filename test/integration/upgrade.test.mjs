import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  statSync,
} from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const version = JSON.parse(readFileSync(path.join(root, 'package.json'), 'utf8')).version;
const development = version === '0.0.0';
const os = { darwin: 'darwin', linux: 'linux' }[process.platform];
const arch = { x64: 'amd64', arm64: 'arm64' }[process.arch];
const asset = `playwright-cli-${os}-${arch}`;
const built = path.join(root, 'target/debug/playwright-cli');
const sha256 = (data) => createHash('sha256').update(data).digest('hex');
const script = (reported) => Buffer.from(`#!/bin/sh\necho "playwright-cli ${reported}"\n`);

function fakeGitHub() {
  const state = {
    tag: 'v9.9.9',
    payload: script('9.9.9'),
    sums: null,
    status: 200,
    downloads: 0,
    requests: [],
  };
  const server = createServer((req, res) => {
    state.requests.push({ url: req.url, agent: req.headers['user-agent'] });
    if (state.status !== 200) {
      res.writeHead(state.status).end('unavailable');
      return;
    }
    const base = `http://127.0.0.1:${server.address().port}`;
    if (req.url === '/repos/ai-ecoverse/slicc-cdp/releases/latest') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(
        JSON.stringify({
          tag_name: state.tag,
          assets: [
            { name: `curlwright-${os}-${arch}`, browser_download_url: `${base}/dl/curlwright` },
            { name: asset, browser_download_url: `${base}/dl/${asset}` },
            { name: 'SHA256SUMS', browser_download_url: `${base}/dl/SHA256SUMS` },
          ],
        })
      );
      return;
    }
    if (req.url === `/dl/${asset}`) {
      res.writeHead(302, { location: `/objects/${asset}` }).end();
      return;
    }
    if (req.url === `/objects/${asset}`) {
      state.downloads += 1;
      res.writeHead(200, { 'content-type': 'application/octet-stream' });
      res.end(state.payload);
      return;
    }
    if (req.url === '/dl/SHA256SUMS') {
      const sums =
        state.sums ??
        `${'0'.repeat(64)}  curlwright-${os}-${arch}\n${sha256(state.payload)}  ${asset}\n`;
      res.writeHead(200, { 'content-type': 'text/plain' });
      res.end(sums);
      return;
    }
    res.writeHead(404).end();
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () =>
      resolve({ state, server, base: `http://127.0.0.1:${server.address().port}` })
    );
  });
}

function run(bin, args, base) {
  return new Promise((resolve, reject) => {
    const env = { ...process.env, SLICC_UPDATE_API_BASE: base };
    delete env.CARGO_HOME;
    const child = spawn(bin, args, { env });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (chunk) => {
      stdout += chunk;
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
    });
    child.on('error', reject);
    child.on('close', (code) => resolve({ code, stdout, stderr }));
  });
}

test('playwright-cli upgrade replaces the binary with the verified latest release', {
  skip: !os || !arch ? `no release asset for ${process.platform} ${process.arch}` : false,
}, async (t) => {
  const scratch = mkdtempSync(path.join(tmpdir(), 'slicc-cdp-upgrade-'));
  const fake = await fakeGitHub();
  t.after(() => {
    fake.server.close();
    rmSync(scratch, { recursive: true, force: true });
  });
  let n = 0;
  const install = () => {
    const dir = path.join(scratch, `bin-${++n}`);
    mkdirSync(dir);
    const bin = path.join(dir, 'playwright-cli');
    copyFileSync(built, bin);
    chmodSync(bin, 0o755);
    return bin;
  };
  const reset = () => {
    Object.assign(fake.state, {
      tag: 'v9.9.9',
      payload: script('9.9.9'),
      sums: null,
      status: 200,
      downloads: 0,
      requests: [],
    });
  };
  const untouched = (bin) => {
    assert.equal(sha256(readFileSync(bin)), sha256(readFileSync(built)));
    assert.deepEqual(readdirSync(path.dirname(bin)), ['playwright-cli']);
  };

  await t.test('--check reports the latest release and changes nothing', async () => {
    reset();
    const bin = install();
    const result = await run(bin, ['upgrade', '--check'], fake.base);
    assert.equal(result.code, 0, result.stderr);
    assert.equal(
      result.stdout,
      development
        ? `latest release: 9.9.9 (this is a development build, ${version})\n`
        : `playwright-cli 9.9.9 is available (you have ${version}); run \`playwright-cli upgrade\`\n`
    );
    assert.equal(fake.state.downloads, 0);
    assert.match(fake.state.requests[0].agent, /^playwright-cli\//);
    untouched(bin);
  });

  await t.test(
    'a development build is not replaced without --force',
    {
      skip: development ? false : 'this build has a release version',
    },
    async () => {
      reset();
      const bin = install();
      const result = await run(bin, ['upgrade'], fake.base);
      assert.equal(result.code, 1);
      assert.equal(
        result.stderr,
        `playwright-cli upgrade: refusing to replace a development build (${version}) with release 9.9.9; pass --force to replace it\n`
      );
      assert.equal(fake.state.downloads, 0);
      untouched(bin);
    }
  );

  await t.test(
    'update is an alias that downloads through a redirect and swaps in place',
    async () => {
      reset();
      const bin = install();
      const result = await run(bin, ['update', '--force'], fake.base);
      assert.equal(result.code, 0, result.stderr);
      assert.equal(
        result.stdout,
        `upgrading playwright-cli ${version} to 9.9.9\nupgraded ${realpathSync(bin)} to playwright-cli 9.9.9 (SHA256SUMS verified)\n`
      );
      assert.equal(fake.state.downloads, 1);
      assert.equal(readFileSync(bin, 'utf8'), script('9.9.9').toString());
      assert.ok(statSync(bin).mode & 0o100);
      assert.deepEqual(readdirSync(path.dirname(bin)), ['playwright-cli']);
      const after = await run(bin, ['--version'], fake.base);
      assert.equal(after.stdout, 'playwright-cli 9.9.9\n');
    }
  );

  await t.test('a real release binary passes the version gate', async () => {
    reset();
    fake.state.tag = `v${version}`;
    fake.state.payload = readFileSync(built);
    const bin = install();
    const before = statSync(bin).ino;
    const upgraded = await run(bin, ['upgrade', '--force'], fake.base);
    assert.equal(upgraded.code, 0, upgraded.stderr);
    assert.equal(fake.state.downloads, 1);
    assert.notEqual(statSync(bin).ino, before);
    untouched(bin);
  });

  await t.test('a checksum mismatch replaces nothing', async () => {
    reset();
    fake.state.sums = `${'ab'.repeat(32)}  ${asset}\n`;
    const bin = install();
    const result = await run(bin, ['upgrade', '--force'], fake.base);
    assert.equal(result.code, 1);
    assert.equal(
      result.stderr,
      `playwright-cli upgrade: ${asset} does not match its SHA-256 in SHA256SUMS\n`
    );
    untouched(bin);
  });

  await t.test('an asset missing from SHA256SUMS is refused before it is downloaded', async () => {
    reset();
    fake.state.sums = `${'0'.repeat(64)}  curlwright-${os}-${arch}\n`;
    const bin = install();
    const result = await run(bin, ['upgrade', '--force'], fake.base);
    assert.equal(result.code, 1);
    assert.equal(result.stderr, `playwright-cli upgrade: SHA256SUMS has no entry for ${asset}\n`);
    assert.equal(fake.state.downloads, 0);
    untouched(bin);
  });

  await t.test('a binary that reports another version replaces nothing', async () => {
    reset();
    fake.state.payload = script('9.9.8');
    const bin = install();
    const result = await run(bin, ['upgrade', '--force'], fake.base);
    assert.equal(result.code, 1);
    assert.equal(
      result.stderr,
      'playwright-cli upgrade: the downloaded binary reports "playwright-cli 9.9.8", expected "playwright-cli 9.9.9"\n'
    );
    untouched(bin);
  });

  await t.test('a binary that does not run replaces nothing', async () => {
    reset();
    fake.state.payload = Buffer.from('not a binary');
    const bin = install();
    const result = await run(bin, ['upgrade', '--force'], fake.base);
    assert.equal(result.code, 1);
    assert.match(
      result.stderr,
      /^playwright-cli upgrade: the downloaded binary failed to run --version/
    );
    untouched(bin);
  });

  await t.test(
    'a binary that is not writable is not downloaded or replaced',
    {
      skip: process.getuid?.() === 0 ? 'root can write anything' : false,
    },
    async () => {
      reset();
      const bin = install();
      chmodSync(bin, 0o555);
      const file = await run(bin, ['upgrade', '--force'], fake.base);
      assert.equal(file.code, 1);
      assert.match(
        file.stderr,
        /^playwright-cli upgrade: .*playwright-cli is not writable; rerun with permission to write it, or reinstall with install\.sh\n$/
      );
      chmodSync(bin, 0o755);
      chmodSync(path.dirname(bin), 0o555);
      try {
        const dir = await run(bin, ['upgrade', '--force'], fake.base);
        assert.equal(dir.code, 1);
        assert.match(dir.stderr, /is not writable, so .*playwright-cli can't be replaced/);
      } finally {
        chmodSync(path.dirname(bin), 0o755);
      }
      assert.equal(fake.state.downloads, 0);
      untouched(bin);
      const check = await run(bin, ['upgrade', '--check'], fake.base);
      assert.equal(check.code, 0, check.stderr);
    }
  );

  await t.test('a cargo install is left to cargo', async () => {
    reset();
    const home = path.join(scratch, 'cargo-home');
    mkdirSync(path.join(home, 'bin'), { recursive: true });
    const bin = path.join(home, 'bin', 'playwright-cli');
    copyFileSync(built, bin);
    const result = await new Promise((resolve, reject) => {
      const child = spawn(bin, ['upgrade'], {
        env: { ...process.env, SLICC_UPDATE_API_BASE: fake.base, CARGO_HOME: home },
      });
      let stderr = '';
      child.stderr.on('data', (chunk) => {
        stderr += chunk;
      });
      child.on('error', reject);
      child.on('close', (code) => resolve({ code, stderr }));
    });
    assert.equal(result.code, 1);
    assert.equal(
      result.stderr,
      'playwright-cli upgrade: playwright-cli was installed with cargo; upgrade it with `cargo install slicc-cdp`\n'
    );
    assert.equal(fake.state.requests.length, 0);
  });

  await t.test('a GitHub API failure is reported', async () => {
    reset();
    fake.state.status = 403;
    const bin = install();
    const result = await run(bin, ['upgrade', '--check'], fake.base);
    assert.equal(result.code, 1);
    assert.equal(
      result.stderr,
      'playwright-cli upgrade: asking GitHub for the latest release: HTTP 403\n'
    );
    untouched(bin);
  });

  await t.test(
    'unknown options and arguments are refused, and help documents both names',
    async () => {
      reset();
      const bin = install();
      const option = await run(bin, ['upgrade', '--nope'], fake.base);
      assert.equal(option.code, 1);
      assert.equal(option.stderr, 'Unknown option: --nope\n');
      const argument = await run(bin, ['update', 'now'], fake.base);
      assert.equal(argument.code, 1);
      assert.match(argument.stderr, /too many arguments/);
      for (const name of ['upgrade', 'update']) {
        const help = await run(bin, [name, '--help'], fake.base);
        assert.equal(help.code, 0);
        assert.match(
          help.stdout,
          new RegExp(`^playwright-cli ${name} \\[--check\\] \\[--force\\]\\n`)
        );
      }
      const global = await run(bin, ['--help'], fake.base);
      assert.match(
        global.stdout,
        /upgrade \[--check\] +replace this binary with the latest release \(alias: update\)/
      );
      assert.equal(fake.state.requests.length, 0);
      untouched(bin);
    }
  );
});
