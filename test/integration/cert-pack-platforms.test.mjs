import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const control = process.env.SLICC_CERT_TOOL_SOURCE ?? root;

for (const target of [
  'x86_64-unknown-linux-musl',
  'x86_64-pc-windows-msvc',
  'aarch64-pc-windows-msvc',
]) {
  test(`PR pack CLI accepts ${target} and local archives before compilation`, (t) => {
    const parent = path.join(root, '.cert/platform-fixtures');
    mkdirSync(parent, { recursive: true });
    const directory = mkdtempSync(
      path.join(parent, process.platform === 'win32' ? 'cli-' : 'D:cli-')
    );
    t.after(() => rmSync(directory, { recursive: true, force: true }));
    const command = (program, args) => {
      const result = spawnSync(program, args, { cwd: directory, encoding: 'utf8', timeout: 30000 });
      assert.equal(result.status, 0, result.stderr);
      return result.stdout.trim();
    };
    const save = (file, value) => writeFileSync(path.join(directory, file), JSON.stringify(value));
    mkdirSync(path.join(directory, 'scripts/cert'), { recursive: true });
    for (const file of ['common.mjs', 'pr-common.mjs', 'pr-pack.mjs'])
      copyFileSync(
        path.join(control, 'scripts/cert', file),
        path.join(directory, 'scripts/cert', file)
      );
    save('package.json', { name: '@ai-ecoverse/slicc-cdp', version: '0.0.0', type: 'module' });
    save('package-lock.json', { version: '0.0.0', packages: { '': { version: '0.0.0' } } });
    writeFileSync(path.join(directory, 'Cargo.toml'), 'fixture = [\n');
    command('git', ['init', '--quiet']);
    command('git', ['add', 'package.json', 'package-lock.json', 'Cargo.toml', 'scripts']);
    command('git', [
      '-c',
      'commit.gpgsign=false',
      '-c',
      'user.name=Fixture',
      '-c',
      'user.email=fixture@example.invalid',
      'commit',
      '--quiet',
      '-m',
      'fixture',
    ]);
    const commit = command('git', ['rev-parse', 'HEAD']);
    save('plan.json', {
      mode: 'pr',
      repository: 'ai-ecoverse/slicc-cdp',
      pr: 1,
      head: commit,
      base: commit,
      tools: commit,
      commit,
      version: '0.0.0-pr.1',
      baseline: { commit, version: '0.0.0-base.1' },
      rust: path.join(directory, 'intentionally-absent-toolchain'),
      run: '1',
      attempt: '1',
    });
    const result = spawnSync(
      process.execPath,
      [
        'scripts/cert/pr-pack.mjs',
        '--plan',
        'plan.json',
        '--side',
        'candidate',
        '--target',
        target,
        '--label',
        'probe',
        '--out',
        'output',
      ],
      {
        cwd: directory,
        encoding: 'utf8',
        timeout: 30000,
        env: { ...process.env, RUSTUP_AUTO_INSTALL: '0' },
      }
    );
    assert.equal(result.signal, null, result.stderr);
    assert.notEqual(result.status, 0);
    assert.ok(!result.stderr.includes('invalid build identity'), result.stderr);
    assert.ok(!result.stderr.includes('Cannot connect'), result.stderr);
    assert.ok(
      result.stderr.includes(`Command failed: cargo build --locked --release --target ${target}`),
      result.stderr
    );
  });
}
