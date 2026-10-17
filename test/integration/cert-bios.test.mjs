import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const pin = '4025d6aabc2c06add216183064d81914a171cd64';
const packageName = '@ai-ecoverse/slicc-cdp';
const read = (file) => JSON.parse(readFileSync(file));
const save = (file, value) => writeFileSync(file, JSON.stringify(value));
const digest = (file) => createHash('sha256').update(readFileSync(file)).digest('hex');

function fixture(t) {
  const directory = mkdtempSync(path.join(tmpdir(), 'slicc-bios-result-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const packs = path.join(directory, 'packs');
  const plan = {
    mode: 'pr',
    go: true,
    pr: 273,
    head: 'a'.repeat(40),
    commit: 'b'.repeat(40),
    base: 'c'.repeat(40),
    tools: 'c'.repeat(40),
    harness: 'c'.repeat(40),
    rust: '1.99.1',
    vectors: '1.7.6',
    version: '0.0.0-pr.273.42',
    baseline: { version: '0.0.0-base.273.42', commit: 'c'.repeat(40) },
    run: '42',
    attempt: '1',
    repository: 'fixture/lane',
  };
  const results = {};
  for (const side of ['base', 'candidate']) {
    const subject =
      side === 'base' ? plan.baseline : { version: plan.version, commit: plan.commit };
    const contents = path.join(directory, `contents-${side}`);
    const packed = path.join(packs, side);
    mkdirSync(path.join(contents, 'package/bin'), { recursive: true });
    mkdirSync(packed, { recursive: true });
    save(path.join(contents, 'package/package.json'), {
      name: packageName,
      version: subject.version,
      gitHead: subject.commit,
    });
    for (const command of ['playwright-cli', 'curlwright'])
      writeFileSync(path.join(contents, `package/bin/${command}.wasm`), `fixture ${side}`);
    execFileSync('tar', ['-czf', 'package.tgz', '-C', contents, 'package'], { cwd: packed });
    const files = [
      { file: 'package.tgz', package: true, sha256: digest(path.join(packed, 'package.tgz')) },
    ];
    for (const command of ['playwright-cli', 'curlwright']) {
      const file = `${command}-host`;
      writeFileSync(path.join(packed, file), 'never executable');
      files.push({ file, sha256: digest(path.join(packed, file)) });
    }
    save(path.join(packed, 'build.json'), {
      ...plan,
      side,
      subject,
      target: 'wasm32-wasip1',
      files,
    });
    const sha256 = files[0].sha256;
    const artifact = `cert-pr-pack-${plan.pr}-${plan.head}-${plan.attempt}-${side}`;
    results[side] = {
      ok: true,
      run: 'canary',
      biosRef: pin,
      repeat: 3,
      url: 'https://github.com/fixture/lane/actions/runs/42',
      overrides: [{ name: packageName, version: subject.version, sha256 }],
      candidates: [
        {
          package: `${packageName}@${subject.version}`,
          via: 'artifact',
          status: `candidate (artifact 42/${artifact}, sha256 ${sha256})`,
        },
      ],
      counts: { pass: 20, skip: 0, fail: 0, regression: 0, both: 0, flaky: 0 },
      regressions: [],
      both: [],
      flaky: [],
      failures: [],
      missingShards: [],
      unexplainedExits: [],
      artifact: {
        name: `${side}-canary`,
        url: `https://github.com/fixture/lane/actions/runs/42/artifacts/${side === 'base' ? 1 : 2}`,
      },
    };
  }
  save(path.join(directory, 'plan.json'), plan);
  return { directory, packs, plan, results };
}

function invoke(f, script = 'pr-bios.mjs', args = []) {
  for (const side of ['base', 'candidate'])
    save(path.join(f.directory, `${side}.json`), f.results[side]);
  const out = path.join(f.directory, 'bios.json');
  const invocation = spawnSync(
    process.execPath,
    [
      path.join(root, 'scripts/cert', script),
      '--plan',
      path.join(f.directory, 'plan.json'),
      '--out',
      out,
      ...args,
    ],
    { encoding: 'utf8', env: { ...process.env, CERT_PR_PACKS: f.packs } }
  );
  return { ...invocation, document: read(out) };
}

const failure = {
  shard: 'guard',
  test: 'guarded CDP smoke',
  log: 'failure',
  failed: { candidate: '3/3', baseline: '0/3' },
};
const cases = [
  ['two complete bound calls', () => {}, 'PASS'],
  [
    'missing candidate call',
    (f) => {
      f.results.candidate = null;
    },
    'INCOMPLETE',
  ],
  [
    'missing base call',
    (f) => {
      f.results.base = null;
    },
    'INCOMPLETE',
  ],
  [
    'candidate regression despite ok true',
    (f) => {
      f.results.candidate.counts.regression = 1;
      f.results.candidate.regressions = [failure];
    },
    'FAIL',
  ],
  [
    'candidate failing smoke',
    (f) => {
      f.results.candidate.counts.fail = 1;
      f.results.candidate.failures = [failure];
    },
    'FAIL',
  ],
  [
    'failure precedence over missing base',
    (f) => {
      f.results.base = null;
      f.results.candidate.counts.regression = 1;
      f.results.candidate.regressions = [failure];
    },
    'FAIL',
  ],
  [
    'wrong candidate digest',
    (f) => {
      f.results.candidate.overrides[0].sha256 = 'f'.repeat(64);
    },
    'FAIL',
  ],
  [
    'wrong base digest',
    (f) => {
      f.results.base.overrides[0].sha256 = 'f'.repeat(64);
    },
    'FAIL',
  ],
  [
    'wrong labelled artifact',
    (f) => {
      f.results.candidate.artifact.name = 'base-canary';
    },
    'INCOMPLETE',
  ],
  [
    'wrong caller run',
    (f) => {
      f.results.candidate.url = 'https://github.com/fixture/lane/actions/runs/43';
    },
    'INCOMPLETE',
  ],
  [
    'wrong result repository',
    (f) => {
      f.results.candidate.artifact.url =
        'https://github.com/other/lane/actions/runs/42/artifacts/2';
    },
    'INCOMPLETE',
  ],
  [
    'wrong producer pin',
    (f) => {
      f.results.candidate.biosRef = 'd'.repeat(40);
    },
    'INCOMPLETE',
  ],
  [
    'wrong package version',
    (f) => {
      f.results.candidate.overrides[0].version = '0.0.1';
    },
    'INCOMPLETE',
  ],
  [
    'missing digest status binding',
    (f) => {
      f.results.candidate.candidates[0].status = 'candidate, not certified';
    },
    'INCOMPLETE',
  ],
  [
    'different source artifact binding',
    (f) => {
      f.results.candidate.candidates[0].status = f.results.base.candidates[0].status;
    },
    'INCOMPLETE',
  ],
  [
    'wrong repeat',
    (f) => {
      f.results.candidate.repeat = 1;
    },
    'INCOMPLETE',
  ],
  [
    'missing shard',
    (f) => {
      f.results.candidate.missingShards = ['guard'];
    },
    'INCOMPLETE',
  ],
  [
    'unexplained exit',
    (f) => {
      f.results.candidate.unexplainedExits = [{ shard: 'guard', exit: 1 }];
    },
    'INCOMPLETE',
  ],
  [
    'skip never credited',
    (f) => {
      f.results.candidate.counts.skip = 1;
    },
    'INCOMPLETE',
  ],
  [
    'flaky never credited',
    (f) => {
      f.results.candidate.counts.flaky = 1;
      f.results.candidate.flaky = [failure];
    },
    'INCOMPLETE',
  ],
  [
    'both failures retained as flags',
    (f) => {
      f.results.candidate.counts.both = 1;
      f.results.candidate.both = [failure];
    },
    'PASS',
  ],
  [
    'ok alone insufficient',
    (f) => {
      f.results.candidate = { ok: true };
    },
    'INCOMPLETE',
  ],
  [
    'empty smoke',
    (f) => {
      f.results.candidate.counts.pass = 0;
    },
    'INCOMPLETE',
  ],
  [
    'malformed counts',
    (f) => {
      f.results.candidate.counts.pass = '20';
    },
    'INCOMPLETE',
  ],
  [
    'malformed failure rows',
    (f) => {
      f.results.candidate.counts.both = 1;
      f.results.candidate.both = [null];
    },
    'INCOMPLETE',
  ],
  [
    'counts cannot hide a regression',
    (f) => {
      f.results.candidate.regressions = [failure];
    },
    'INCOMPLETE',
  ],
  [
    'producer refusal',
    (f) => {
      f.results.candidate.ok = false;
    },
    'INCOMPLETE',
  ],
  [
    'duplicate result artifacts',
    (f) => {
      f.results.candidate.artifact.url = f.results.base.artifact.url;
    },
    'INCOMPLETE',
  ],
];

for (const [name, mutate, expected] of cases)
  test(`bios consumer public CLI: ${name}`, (t) => {
    const f = fixture(t);
    mutate(f);
    const result = invoke(f);
    assert.equal(result.document.status, expected, result.stderr);
    assert.equal(result.status, expected === 'FAIL' ? 1 : 0, result.stderr);
    if (name.startsWith('both failures'))
      assert.match(result.document.flags.join(';'), /failures also present on bios pins/);
  });

test('bios input CLI requires both byte-checked labelled packs and repeat three', (t) => {
  const f = fixture(t);
  const { document } = invoke(f, 'pr-bios-inputs.mjs');
  assert.equal(document.ready, true);
  assert.equal(document.pin.commit, pin);
  for (const side of ['base', 'candidate']) {
    assert.equal(document.calls[side].label, side);
    assert.equal(document.calls[side].repeat, 3);
    assert.equal(document.calls[side]['candidate-sha256'], f.results[side].overrides[0].sha256);
  }
});

test('bios consumer refuses corrupted package bytes before accepting canary output', (t) => {
  const f = fixture(t);
  writeFileSync(path.join(f.packs, 'candidate/package.tgz'), 'corrupt bytes');
  const result = invoke(f);
  assert.equal(result.document.status, 'FAIL');
  assert.match(result.document.reason, /pack digest mismatch/);
});
