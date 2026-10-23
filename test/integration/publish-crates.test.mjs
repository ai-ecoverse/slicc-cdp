import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const script = fileURLToPath(new URL('../../scripts/publish-crates.sh', import.meta.url));
const crates = ['slicc-cdp-client', 'slicc-discovery', 'slicc-cdp', 'curlwright'];

function publish(t, failures) {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cdp-publish-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  mkdirSync(path.join(dir, 'scripts'));
  mkdirSync(path.join(dir, 'bin'));
  copyFileSync(script, path.join(dir, 'scripts/publish-crates.sh'));
  for (const file of ['set-crate-versions.mjs', 'crate-readmes.mjs'])
    writeFileSync(path.join(dir, 'scripts', file), '');
  writeFileSync(path.join(dir, 'failures.json'), JSON.stringify(failures));
  const fake = (name, body) => {
    writeFileSync(path.join(dir, 'bin', name), body);
    chmodSync(path.join(dir, 'bin', name), 0o755);
  };
  fake(
    'cargo',
    `#!${process.execPath}
const fs = require('node:fs');
const args = process.argv.slice(2);
if (args[0] !== 'publish') process.exit(0);
const crate = args[args.indexOf('-p') + 1];
fs.appendFileSync('published.log', crate + '\\n');
const failure = JSON.parse(fs.readFileSync('failures.json', 'utf8'))[crate];
if (failure) { process.stderr.write(failure); process.exit(101); }
process.stdout.write('Uploaded ' + crate + '\\n');
`
  );
  fake('sleep', '#!/bin/sh\nexit 0\n');
  const result = spawnSync('sh', ['scripts/publish-crates.sh', '1.35.0'], {
    cwd: dir,
    encoding: 'utf8',
    env: { ...process.env, PATH: `${path.join(dir, 'bin')}:${process.env.PATH}` },
  });
  let attempts = [];
  try {
    attempts = readFileSync(path.join(dir, 'published.log'), 'utf8').trim().split('\n');
  } catch {}
  return { ...result, attempts };
}

const limited =
  'error: failed to publish slicc-cdp-client v1.35.0 to registry at https://crates.io\n\nCaused by:\n  the remote server responded with an error (status 429 Too Many Requests): You have published too many versions of this crate in the last 24 hours\n';

test('every crate publishes in order when crates.io accepts them', (t) => {
  const result = publish(t, {});
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.attempts, crates);
  assert.match(result.stdout, /^publish-crates: PASS 4 crates 1\.35\.0$/m);
});

test('a crates.io rate limit warns, skips the rest of the release once, and passes', (t) => {
  const result = publish(t, { 'slicc-cdp-client': limited });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.attempts, ['slicc-cdp-client'], 'no retry and no later crate');
  assert.match(
    result.stdout,
    /^::warning::crates\.io rate-limited slicc-cdp-client 1\.35\.0; skipped it and the crates after it/m
  );
  assert.match(
    result.stdout,
    /^publish-crates: SKIP slicc-cdp-client 1\.35\.0 rate-limited by crates\.io$/m
  );
  assert.doesNotMatch(result.stdout, /publish-crates: PASS/);
});

test('a rate limit on a later crate keeps the ones already published and skips the rest', (t) => {
  const result = publish(t, { 'slicc-cdp': limited.replaceAll('slicc-cdp-client', 'slicc-cdp') });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.attempts, ['slicc-cdp-client', 'slicc-discovery', 'slicc-cdp']);
  assert.match(result.stdout, /^::warning::crates\.io rate-limited slicc-cdp 1\.35\.0/m);
});

test('any other publish error still fails the release', (t) => {
  const result = publish(t, {
    'slicc-discovery':
      'error: failed to publish slicc-discovery v1.35.0\n\nCaused by:\n  the remote server responded with an error (status 403 Forbidden): this token cannot publish\n',
  });
  assert.notEqual(result.status, 0);
  assert.equal(result.attempts.filter((name) => name === 'slicc-discovery').length, 8);
  assert.ok(!result.attempts.includes('slicc-cdp'), 'stops at the failed crate');
  assert.doesNotMatch(result.stdout, /::warning::|publish-crates: (PASS|SKIP)/);
});

test('an already uploaded version counts as published', (t) => {
  const result = publish(t, {
    'slicc-cdp-client': 'error: crate version `1.35.0` is already uploaded\n',
  });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.attempts, crates);
});
