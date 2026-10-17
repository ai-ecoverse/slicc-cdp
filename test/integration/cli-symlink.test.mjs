import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const version = JSON.parse(readFileSync(path.join(root, 'package.json'), 'utf8')).version;
const windows = process.platform === 'win32';

function layout() {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cdp-cli-link-'));
  const pkg = path.join(dir, 'node_modules', '@ai-ecoverse', 'slicc-cdp');
  mkdirSync(path.dirname(pkg), { recursive: true });
  symlinkSync(root, pkg, windows ? 'junction' : 'dir');
  const bin = path.join(dir, 'node_modules', '.bin');
  mkdirSync(bin, { recursive: true });
  if (!windows) {
    symlinkSync(
      path.join('..', '@ai-ecoverse', 'slicc-cdp', 'src', 'cli.js'),
      path.join(bin, 'slicc-cdp')
    );
    symlinkSync(path.join(root, 'src', 'cli.js'), path.join(dir, 'cli-link.js'));
  }
  return { dir, pkg, bin };
}

function expectVersion(args) {
  const result = spawnSync(process.execPath, [...args, '--version'], { encoding: 'utf8' });
  assert.equal(result.status, 0, `${args.join(' ')}: ${result.stderr}`);
  assert.equal(
    result.stdout,
    `slicc-cdp ${version}\n`,
    `${args.join(' ')} printed ${JSON.stringify(result.stdout)}`
  );
}

test('the CLI runs through a symlinked package dir, with and without --preserve-symlinks-main', () => {
  const { dir, pkg } = layout();
  try {
    const cli = path.join(pkg, 'src', 'cli.js');
    expectVersion([cli]);
    expectVersion(['--preserve-symlinks-main', cli]);
    expectVersion(['--preserve-symlinks', '--preserve-symlinks-main', cli]);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('the CLI runs through a symlink to cli.js and through an npm-style bin link', {
  skip: windows,
}, () => {
  const { dir, bin } = layout();
  try {
    expectVersion([path.join(dir, 'cli-link.js')]);
    expectVersion([path.join(bin, 'slicc-cdp')]);
    expectVersion(['--preserve-symlinks', path.join(bin, 'slicc-cdp')]);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
