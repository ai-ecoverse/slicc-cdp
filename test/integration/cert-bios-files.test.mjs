import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { Readable } from 'node:stream';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const writer = path.join(root, 'scripts/cert/pr-bios-files.mjs');
const fixtureFile = path.join(root, 'scripts/cert/pr-bios-file-fixture.mjs');
const library = existsSync(writer) ? await import(writer) : null;
const fixture = existsSync(fixtureFile) ? await import(fixtureFile) : null;
const workflow = path.join(root, '.github/workflows/cert-bios-file-self-test.yml');
function dir(t) {
  const value = mkdtempSync(path.join(tmpdir(), 'slicc-bios-files-'));
  t.after(() => rmSync(value, { recursive: true, force: true }));
  return value;
}
function tools() {
  assert.ok(library && fixture, 'main-owned bounded file writer and fixture must exist');
  return { ...library, ...fixture };
}
const envelope = () => ({ base: '{"side":"base"}', candidate: '{"side":"candidate"}' });

for (const bytes of [150 * 1024, 160 * 1024, 512 * 1024]) {
  test(`bios file writer preserves both complete ${bytes}-byte results`, (t) => {
    const { writeBiosFiles, biosFileFixture } = tools();
    const input = Object.fromEntries(
      ['base', 'candidate'].map((side) => [side, biosFileFixture(side, bytes)])
    );
    const directory = path.join(dir(t), 'files');
    const result = writeBiosFiles(input, directory);
    assert.equal(result.status, 'PASS');
    for (const side of ['base', 'candidate']) {
      const file = readFileSync(path.join(directory, result.files[side].file));
      assert.equal(file.length, bytes);
      assert.equal(file.toString(), input[side]);
      assert.equal(result.files[side].bytes, bytes);
    }
  });
}

for (const [name, input, status] of [
  ['missing base', { candidate: '{}' }, 'INCOMPLETE'],
  ['missing candidate', { base: '{}' }, 'INCOMPLETE'],
  ['null result', { base: null, candidate: '{}' }, 'INCOMPLETE'],
  ['empty result', { base: '', candidate: '{}' }, 'INCOMPLETE'],
  ['null JSON result', { base: 'null', candidate: '{}' }, 'INCOMPLETE'],
  ['numeric result', { base: 7, candidate: '{}' }, 'FAIL'],
  ['malformed result', { base: '{', candidate: '{}' }, 'FAIL'],
  ['array result', { base: '[]', candidate: '{}' }, 'FAIL'],
  ['scalar result', { base: 'true', candidate: '{}' }, 'FAIL'],
  ['unexpected role', { ...envelope(), outside: '{}' }, 'FAIL'],
  ['array envelope', [], 'FAIL'],
  ['null envelope', null, 'FAIL'],
]) {
  test(`bios file writer refuses ${name} without partial output`, (t) => {
    const { writeBiosFiles } = tools();
    const directory = path.join(dir(t), 'files');
    const result = writeBiosFiles(input, directory);
    assert.equal(result.status, status);
    assert.equal(existsSync(directory), false);
    assert.deepEqual(result.files, {});
  });
}

test('oversized candidate never truncates or writes a healthy base first', (t) => {
  const { writeBiosFiles, biosFileFixture, sideLimit } = tools();
  const directory = path.join(dir(t), 'files');
  const result = writeBiosFiles(
    { base: '{}', candidate: biosFileFixture('candidate', sideLimit + 1) },
    directory
  );
  assert.equal(result.status, 'INCOMPLETE');
  assert.equal(existsSync(directory), false);
});

for (const [name, chunks, status] of [
  ['empty stream', [], 'INCOMPLETE'],
  ['invalid UTF-8', [Buffer.from([255])], 'FAIL'],
  ['invalid envelope JSON', ['{'], 'FAIL'],
]) {
  test(`bios stdin transport rejects ${name}`, async () => {
    const { biosEnvelope } = tools();
    assert.equal((await biosEnvelope(Readable.from(chunks))).status, status);
  });
}

test('bios stdin cap drains but never parses a truncated prefix', async () => {
  const { biosEnvelope, inputLimit } = tools();
  const input = 'x'.repeat(inputLimit + 1);
  const result = await biosEnvelope(Readable.from([input.slice(0, 100), input.slice(100)]));
  assert.equal(result.status, 'INCOMPLETE');
  assert.equal(result.value, undefined);
});

test('bios stdin decoding preserves multibyte data across chunk boundaries', async () => {
  const { biosEnvelope } = tools();
  const input = { base: '{"text":"😀"}', candidate: '{}' };
  const bytes = Buffer.from(JSON.stringify(input));
  const result = await biosEnvelope(
    Readable.from(Array.from(bytes, (byte) => Buffer.from([byte])))
  );
  assert.deepEqual(result.value, input);
});

for (const kind of ['regular file', 'symbolic link', 'directory link']) {
  test(`bios file writer never overwrites or follows an existing ${kind}`, (t) => {
    const { writeBiosFiles } = tools();
    const directory = dir(t);
    const outside = path.join(directory, 'outside');
    mkdirSync(outside);
    const target = path.join(outside, 'target.json');
    writeFileSync(target, 'untouched');
    const output = path.join(directory, 'files');
    if (kind === 'directory link') symlinkSync(outside, output, 'dir');
    else {
      mkdirSync(output);
      if (kind === 'symbolic link') symlinkSync(target, path.join(output, 'bios-base.json'));
      else writeFileSync(path.join(output, 'bios-base.json'), 'existing');
    }
    const result = writeBiosFiles(envelope(), output);
    assert.equal(result.status, 'FAIL');
    assert.equal(readFileSync(target, 'utf8'), 'untouched');
    assert.equal(existsSync(path.join(output, 'bios-candidate.json')), false);
  });
}

test('main self-test expands 160 KiB per side through a quoted script-file heredoc', (t) => {
  const { biosFileFixture, fixtureBytes } = tools();
  assert.ok(existsSync(workflow), 'main-only live size proof workflow must exist');
  const yaml = readFileSync(workflow, 'utf8');
  assert.ok(yaml.length < 21000);
  assert.match(yaml, /workflow_dispatch:/);
  assert.doesNotMatch(
    yaml,
    /pull_request:|push:|secrets\.|BIOS_BASE:|BIOS_CANDIDATE:|github-script@/
  );
  assert.match(yaml, /contents: read/);
  assert.match(yaml, /if: github.ref == 'refs\/heads\/main'/);
  const match = /name: Expand JSON strings[^\n]*\n {8}run: \|\n([\s\S]*?) {6}- name: Check/.exec(
    yaml
  );
  assert.ok(match);
  const input = Object.fromEntries(
    ['base', 'candidate'].map((side) => [side, biosFileFixture(side)])
  );
  let script = match[1]
    .split('\n')
    .map((line) => line.slice(10))
    .join('\n');
  for (const side of ['base', 'candidate'])
    script = script.replace(
      `\u0024{{ toJSON(steps.fixture.outputs.${side}) }}`,
      JSON.stringify(input[side])
    );
  script = script
    .replaceAll('node scripts/', `"${process.execPath}" "${root}scripts/`)
    .replaceAll('.mjs ', '.mjs" ');
  assert.match(script, /<<'EOF'/);
  assert.doesNotMatch(script, /\u0024\{\{/);
  const directory = dir(t);
  mkdirSync(path.join(directory, 'evidence'));
  const file = path.join(directory, 'expanded.sh');
  writeFileSync(file, script);
  const run = spawnSync('bash', ['--noprofile', '--norc', '-e', '-o', 'pipefail', file], {
    cwd: directory,
    encoding: 'utf8',
    timeout: 10000,
    env: { PATH: process.env.PATH, HOME: process.env.HOME, SLICC_CDP_PORTS: '9300-9309' },
  });
  assert.equal(run.status, 0, run.stderr);
  assert.match(run.stdout, /pr-bios-files: PASS/);
  for (const side of ['base', 'candidate']) {
    const bytes = readFileSync(path.join(directory, 'evidence', `bios-${side}.json`));
    assert.equal(bytes.length, fixtureBytes);
    assert.equal(bytes.toString(), input[side]);
  }
  assert.equal(existsSync(path.join(directory, 'injected')), false);
});

test('Linux confirms why a 160 KiB single environment value is not a file transport', {
  skip: process.platform !== 'linux',
}, () => {
  const { biosFileFixture } = tools();
  const run = spawnSync(process.execPath, ['-e', 'process.exit(0)'], {
    env: { BIOS_BASE: biosFileFixture('base') },
  });
  assert.equal(run.error?.code, 'E2BIG');
  assert.equal(run.status, null);
});

test('bios file CLI reports missing stdin once and writes no result files', (t) => {
  tools();
  const directory = path.join(dir(t), 'files');
  const run = spawnSync(process.execPath, [writer, '--directory', directory], {
    input: '',
    encoding: 'utf8',
  });
  assert.equal(run.status, 1);
  assert.equal(run.stdout.match(/^pr-bios-files: INCOMPLETE/gm)?.length, 1);
  assert.equal(existsSync(directory), false);
});
