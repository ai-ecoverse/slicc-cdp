import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdtempSync, readFileSync, rmSync, unlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';

const attributes = new URL('../../.gitattributes', import.meta.url);
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');

test('public Git checkout and archive preserve source lock bytes with Windows autocrlf', (t) => {
  const directory = mkdtempSync(path.join(tmpdir(), 'slicc-source-text-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const git = (...args) =>
    execFileSync('git', ['-c', 'commit.gpgsign=false', ...args], { cwd: directory });
  git('init', '--quiet', '--initial-branch=main');
  git('config', 'user.name', 'Fixture');
  git('config', 'user.email', 'fixture@example.invalid');
  git('config', 'core.autocrlf', 'false');
  const original = Buffer.from('version = 4\n\n[[package]]\nname = "fixture"\nversion = "0.0.0"\n');
  const file = path.join(directory, 'Cargo.lock');
  writeFileSync(file, original);
  git('add', 'Cargo.lock');
  git('commit', '--quiet', '-m', 'canonical source');
  git('config', 'core.autocrlf', 'true');
  unlinkSync(file);
  git('checkout', 'HEAD', '--', 'Cargo.lock');
  assert.ok(
    readFileSync(file).includes(Buffer.from('\r\n')),
    'control creates a real CRLF working tree'
  );
  if (existsSync(attributes)) {
    writeFileSync(path.join(directory, '.gitattributes'), readFileSync(attributes));
    git('add', '.gitattributes');
    git('commit', '--quiet', '-m', 'preserve source text bytes');
  }
  unlinkSync(file);
  git('checkout', 'HEAD', '--', 'Cargo.lock');
  const blob = git('show', 'HEAD:Cargo.lock');
  assert.equal(
    digest(readFileSync(file)),
    digest(blob),
    'checkout lock digest must equal the immutable blob'
  );
  assert.equal(digest(blob), digest(original));
  writeFileSync(path.join(directory, 'source.tar'), git('archive', '--format=tar', 'HEAD'));
  const archived = execFileSync('tar', ['-xOf', 'source.tar', 'Cargo.lock'], { cwd: directory });
  assert.equal(digest(archived), digest(blob), 'archive lock digest must equal the immutable blob');
});
