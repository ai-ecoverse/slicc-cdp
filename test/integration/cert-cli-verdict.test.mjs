import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { prIdentity } from '../../scripts/cert/pr-common.mjs';
import { assertVerdict } from './cert-cli-verdict.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const names = [
  'pr-plan',
  'pr-proofs',
  'pr-pack',
  'pr-native',
  'pr-sign-output',
  'pr-bios-inputs',
  'pr-bios',
  'pr-review',
  'pr-record',
  'pr-report',
  'rebuild',
  'guard',
  'integration',
  'assets',
];
const save = (file, document) => writeFileSync(file, JSON.stringify(document));
const read = (file) => JSON.parse(readFileSync(file));
const digest = (file) => createHash('sha256').update(readFileSync(file)).digest('hex');
function directory(t) {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cli-verdict-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}
function cli(name, argv) {
  return spawnSync(process.execPath, [path.join(root, `scripts/cert/${name}.mjs`), ...argv], {
    encoding: 'utf8',
    timeout: 10000,
    env: {
      ...process.env,
      GH_TOKEN: '',
      GITHUB_OUTPUT: '',
      GITHUB_STEP_SUMMARY: '',
      CERT_PR_PACKS: '',
      SLICC_CDP_PORTS: '9300-9309',
    },
  });
}

for (const name of names) {
  for (const refusal of ['unknown argument', 'malformed input']) {
    test(`${name} prints exactly one verdict on early ${refusal}`, (t) => {
      const dir = directory(t);
      writeFileSync(path.join(dir, 'plan.json'), '{');
      const flag = ['pr-plan', 'pr-report'].includes(name)
        ? '--event'
        : name === 'pr-record'
          ? '--evidence'
          : '--plan';
      const run = cli(
        name,
        refusal === 'unknown argument'
          ? ['--unknown-verdict-fixture']
          : [
              flag,
              name === 'pr-record' ? dir : path.join(dir, 'plan.json'),
              '--out',
              path.join(dir, 'result.json'),
            ]
      );
      assert.equal(run.status, 1, run.stderr);
      assert.match(run.stderr, refusal === 'unknown argument' ? /unknown-verdict-fixture/ : /JSON/);
      assertVerdict(
        run.stdout,
        name === 'assets'
          ? process.platform === 'darwin'
            ? 'assets-macos'
            : 'assets-linux'
          : name,
        'INCOMPLETE'
      );
    });
  }
}

for (const [name, action] of [
  ['pr-native', 'sign-input'],
  ['pr-native', 'collect'],
  ['pr-report', 'pr'],
  ['pr-report', 'group'],
]) {
  test(`${name} ${action} prints exactly one early refusal verdict`, () => {
    const run = cli(name, ['--action', action]);
    assert.equal(run.status, 1, run.stderr);
    assertVerdict(run.stdout, name, 'INCOMPLETE');
  });
}

for (const status of ['PASS', 'FAIL', 'INCOMPLETE']) {
  for (const throws of [false, true]) {
    test(`common evidence prints its ${status} document status${throws ? ' on caught refusal' : ''}`, (t) => {
      const dir = directory(t);
      const source = path.join(dir, 'evidence.mjs');
      const body = throws
        ? `const error = new Error('fixture refusal'); error.certStatus = '${status}'; throw error;`
        : `return { status: '${status}' };`;
      writeFileSync(
        source,
        `import { evidence } from ${JSON.stringify(new URL('../../scripts/cert/common.mjs', import.meta.url).href)};\nawait evidence('fixture', async () => { ${body} });\n`
      );
      save(path.join(dir, 'plan.json'), {});
      const out = path.join(dir, 'result.json');
      const run = spawnSync(
        process.execPath,
        [source, '--plan', path.join(dir, 'plan.json'), '--out', out],
        {
          encoding: 'utf8',
        }
      );
      assert.equal(run.status, 0, run.stderr);
      const expected = throws && status !== 'FAIL' ? 'INCOMPLETE' : status;
      assert.equal(read(out).status, expected);
      assertVerdict(run.stdout, 'fixture', expected);
    });
  }
}

for (const exit of [0, 7]) {
  test(`common evidence prints INCOMPLETE on silent early exit ${exit} without creating a document`, (t) => {
    const dir = directory(t);
    const source = path.join(dir, 'silent.mjs');
    writeFileSync(
      source,
      `import { evidence } from ${JSON.stringify(new URL('../../scripts/cert/common.mjs', import.meta.url).href)};\nawait evidence('fixture', async () => process.exit(${exit}));\n`
    );
    save(path.join(dir, 'plan.json'), {});
    const out = path.join(dir, 'result.json');
    const run = spawnSync(
      process.execPath,
      [source, '--plan', path.join(dir, 'plan.json'), '--out', out],
      { encoding: 'utf8' }
    );
    assert.equal(run.status, exit, run.stderr);
    assertVerdict(run.stdout, 'fixture', 'INCOMPLETE');
    assert.throws(() => readFileSync(out), { code: 'ENOENT' });
  });
}

test('PR proof derivation prints the unchanged plan document status', (t) => {
  const dir = directory(t);
  const commit = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim();
  const plan = { mode: 'pr', go: true, status: 'INCOMPLETE', base: commit, commit, pr: 1 };
  const file = path.join(dir, 'plan.json');
  const out = path.join(dir, 'derived-plan.json');
  save(file, plan);
  const run = cli('pr-proofs', ['--plan', file, '--out', out]);
  assert.equal(run.status, 0, run.stderr);
  assert.equal(read(out).status, plan.status);
  assertVerdict(run.stdout, 'pr-proofs', plan.status);
});

test('native preparation and sign-output print operation verdicts without claiming signature checks', (t) => {
  const dir = directory(t);
  const plan = { mode: 'pr', version: 'fixture', commit: 'a'.repeat(40) };
  const file = path.join(dir, 'plan.json');
  save(file, plan);
  const platforms = read(path.join(root, 'scripts/cert/pr-native.json'));
  for (const platform of platforms.filter((item) => item.label.startsWith('darwin-'))) {
    const native = path.join(dir, platform.label);
    mkdirSync(native);
    const files = ['playwright-cli', 'curlwright'].map((command) => {
      const file = `${command}-${platform.label}`;
      writeFileSync(path.join(native, file), Buffer.from('cffaedfe00000000', 'hex'));
      return { file, sha256: digest(path.join(native, file)) };
    });
    save(path.join(native, 'build.json'), {
      ...prIdentity(plan),
      side: 'candidate',
      subject: { commit: plan.commit, version: plan.version },
      ...platform,
      files,
    });
  }
  const prepared = path.join(dir, 'prepared');
  const prepare = cli('pr-native', [
    '--plan',
    file,
    '--directory',
    dir,
    '--action',
    'sign-input',
    '--out',
    prepared,
  ]);
  assert.equal(prepare.status, 0, prepare.stderr);
  assertVerdict(prepare.stdout, 'pr-native', 'PASS');
  assert.equal(read(path.join(prepared, 'sign-input.json')).status, undefined);
  const out = path.join(dir, 'signed');
  const signed = cli('pr-sign-output', ['--plan', file, '--directory', prepared, '--out', out]);
  assert.equal(signed.status, 0, signed.stderr);
  assertVerdict(signed.stdout, 'pr-sign-output', 'PASS');
  assert.equal(read(path.join(out, 'signed.json')).status, undefined);
  const binary = path.join(dir, 'darwin-amd64', 'playwright-cli-darwin-amd64');
  writeFileSync(binary, 'wrong bytes');
  const corrupt = cli('pr-native', [
    '--plan',
    file,
    '--directory',
    dir,
    '--action',
    'sign-input',
    '--out',
    prepared,
  ]);
  assert.equal(corrupt.status, 1, corrupt.stderr);
  assert.match(corrupt.stderr, /native source digest mismatch/);
  assertVerdict(corrupt.stdout, 'pr-native', 'FAIL');
});
