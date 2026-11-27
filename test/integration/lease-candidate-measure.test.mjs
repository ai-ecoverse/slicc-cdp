import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { killTree } from './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const corpus = path.join(root, 'test/integration/lease-scope-v1.candidate.json');
const digest = '0fbec3e0add0af611c196a0ab5cb8918a933db243d7724cfe9df441889e01246';
const length = 151818;

test('the candidate exact-initial corpus measured against the released lease parser', async () => {
  const bytes = readFileSync(corpus);
  const actual = createHash('sha256').update(bytes).digest('hex');
  console.log(`CANDIDATE-VENDORED length=${bytes.length} sha256=${actual}`);
  assert.equal(bytes.length, length);
  assert.equal(actual, digest);
  const parsed = JSON.parse(bytes.toString('utf8'));
  assert.equal(parsed.exactInitialPilotCases.length, 215);
  assert.equal(parsed.grantEchoCases.length, 16);
  assert.equal(parsed.exactInitialPilotReviewCases.length, 8);
  const rustc = version('rustc');
  const cargoVersion = version('cargo');
  const lock = readFileSync(path.join(root, 'Cargo.lock'), 'utf8');
  const url = lock.match(/\[\[package\]\]\nname = "url"\nversion = "([^"]+)"/)?.[1] ?? 'unknown';
  const result = await cargo([
    'test',
    '--locked',
    '-p',
    'slicc-cdp',
    '--lib',
    'lease::corpus_test',
    '--',
    '--nocapture',
    '--test-threads=1',
  ]);
  const lines = result.stdout.split('\n');
  const target = lines
    .find((line) => line.startsWith('CANDIDATE-TARGET '))
    ?.match(/arch=(\S+) os=(\S+)/);
  const triple = target ? `${target[1]}-${target[2]}` : 'unknown';
  const start = lines.findIndex((line) => line.startsWith('CANDIDATE-TARGET '));
  const end = lines.findIndex((line) => line.startsWith('released 2.2.0 behaviour only'));
  const report = start >= 0 && end >= start ? lines.slice(start, end + 1) : lines;
  console.log(
    [
      `CANDIDATE-HEADER rustc=${rustc} cargo=${cargoVersion} url=${url} target=${triple} node=${process.version}`,
      ...report,
    ].join('\n')
  );
  assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
  assert.match(
    result.stdout,
    /^CANDIDATE-COUNTS exactInitialPilotCases=215 grantEchoCases=16 exactInitialPilotReviewCases=8$/m
  );
  const rows = lines.filter((line) => line.startsWith('CANDIDATE-ROW '));
  assert.equal(rows.length, 215);
  for (const row of rows) assert.match(row, / S1=\S+ S2=\S+ S3=\S+$/);
  assert.deepEqual(
    new Set(rows.map((line) => line.match(/ id=(\S+) /)?.[1])),
    new Set(parsed.exactInitialPilotCases.map((row) => row.id))
  );
  assert.match(result.stdout, /^CANDIDATE-EVALUATED rows=215$/m);
});

function version(command) {
  const result = spawnSync(command, ['--version'], { cwd: root, encoding: 'utf8' });
  return result.status === 0 ? result.stdout.trim() : `unavailable (${result.status})`;
}

function cargo(args) {
  return new Promise((resolve, reject) => {
    const env = { ...process.env };
    delete env.SLICC_CDP_URL;
    const child = spawn('cargo', args, { cwd: root, env, detached: process.platform !== 'win32' });
    const out = [];
    const err = [];
    child.stdout.on('data', (chunk) => out.push(chunk));
    child.stderr.on('data', (chunk) => err.push(chunk));
    const timer = setTimeout(() => {
      killTree(child);
      reject(new Error('cargo test timed out'));
    }, 600000);
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
