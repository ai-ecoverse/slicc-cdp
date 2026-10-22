import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const version = JSON.parse(readFileSync(path.join(root, 'package.json'), 'utf8')).version;
const os = { darwin: 'darwin', linux: 'linux' }[process.platform];
const arch = { x64: 'amd64', arm64: 'arm64' }[process.arch];
const bins = ['playwright-cli', 'curlwright'];
const sha256 = (file) => createHash('sha256').update(readFileSync(file)).digest('hex');

function serve(dir) {
  return new Promise((resolve) => {
    const server = createServer((req, res) => {
      const file = path.join(dir, path.basename(decodeURIComponent(req.url)));
      if (!existsSync(file)) {
        res.writeHead(404).end();
        return;
      }
      res.writeHead(200, { 'content-type': 'application/octet-stream' });
      res.end(readFileSync(file));
    });
    server.listen(0, '127.0.0.1', () => resolve(server));
  });
}

function install(base, args) {
  return new Promise((resolve, reject) => {
    const child = spawn('sh', [path.join(root, 'install.sh'), '--version', '9.9.9', ...args], {
      env: { ...process.env, SLICC_INSTALL_BASE_URL: base },
    });
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

function release(dir, { tamper, drop } = {}) {
  for (const bin of bins)
    copyFileSync(path.join(root, 'target/debug', bin), path.join(dir, `${bin}-${os}-${arch}`));
  const sums = bins
    .map((bin) => `${bin}-${os}-${arch}`)
    .filter((asset) => asset !== drop)
    .map((asset) => `${sha256(path.join(dir, asset))}  ${asset}\n`);
  writeFileSync(path.join(dir, 'SHA256SUMS'), sums.join(''));
  if (tamper) writeFileSync(path.join(dir, tamper), 'tampered');
}

test('install.sh installs the release binaries it verified, and refuses a tampered or unlisted one', {
  skip: !os || !arch ? `no release asset for ${process.platform} ${process.arch}` : false,
}, async (t) => {
  const scratch = mkdtempSync(path.join(tmpdir(), 'slicc-cdp-install-'));
  const assets = path.join(scratch, 'assets');
  mkdirSync(assets);
  const server = await serve(assets);
  t.after(() => {
    server.close();
    rmSync(scratch, { recursive: true, force: true });
  });
  const base = `http://127.0.0.1:${server.address().port}`;

  await t.test('installs both binaries, byte for byte, executable, and runs them', async () => {
    release(assets);
    const dir = path.join(scratch, 'bin');
    const result = await install(base, ['--dir', dir]);
    assert.equal(result.code, 0, result.stderr);
    for (const bin of bins) {
      const file = path.join(dir, bin);
      assert.equal(sha256(file), sha256(path.join(root, 'target/debug', bin)));
      assert.ok(statSync(file).mode & 0o100, `${bin} is executable`);
    }
    assert.match(result.stdout, new RegExp(`${version.replaceAll('.', '\\.')}`));
    const again = await install(base, ['--dir', dir]);
    assert.equal(again.code, 0, again.stderr);
    assert.deepEqual(readdirSync(dir).sort(), [...bins].sort());
  });

  await t.test('--only installs one binary', async () => {
    const dir = path.join(scratch, 'only');
    const result = await install(base, ['--dir', dir, '--only', 'curlwright']);
    assert.equal(result.code, 0, result.stderr);
    assert.deepEqual(readdirSync(dir), ['curlwright']);
  });

  await t.test('a tampered asset fails the checksum and installs nothing', async () => {
    release(assets, { tamper: `curlwright-${os}-${arch}` });
    const dir = path.join(scratch, 'tampered');
    const result = await install(base, ['--dir', dir]);
    assert.notEqual(result.code, 0);
    assert.match(result.stderr, new RegExp(`checksum mismatch for curlwright-${os}-${arch}`));
    assert.equal(existsSync(dir), false);
  });

  await t.test('an asset missing from SHA256SUMS is refused', async () => {
    release(assets, { drop: `playwright-cli-${os}-${arch}` });
    const dir = path.join(scratch, 'unlisted');
    const result = await install(base, ['--dir', dir]);
    assert.notEqual(result.code, 0);
    assert.match(result.stderr, new RegExp(`no checksum found for playwright-cli-${os}-${arch}`));
    assert.equal(existsSync(dir), false);
  });
});
