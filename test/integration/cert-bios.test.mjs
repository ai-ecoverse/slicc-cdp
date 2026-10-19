import assert from 'node:assert/strict';
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { artifactNames } from '../../scripts/cert/pr-artifact-binding.mjs';
import * as biosLibrary from '../../scripts/cert/pr-bios-result.mjs';
import { prIdentity } from '../../scripts/cert/pr-common.mjs';
import { assertVerdict } from './cert-cli-verdict.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const pin = '4025d6aabc2c06add216183064d81914a171cd64';
const packageName = '@ai-ecoverse/slicc-cdp';
const read = (file) => JSON.parse(readFileSync(file));
const save = (file, value) => writeFileSync(file, JSON.stringify(value));
const digest = (file) => createHash('sha256').update(readFileSync(file)).digest('hex');
const inputDirectoryArgs = (f, script) =>
  script === 'pr-bios-inputs.mjs' &&
  /\bdirectory:\s*\{/.test(readFileSync(path.join(root, 'scripts/cert', script), 'utf8'))
    ? ['--directory', path.join(f.directory, 'unpacked-inputs')]
    : [];

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
      refused: [],
      ambiguous: [],
      tests: Array.from({ length: 20 }, (_, index) => ({
        key: `guard/test-${index}`,
        shard: index === 19 ? 'packages-update-cdp' : 'guard',
        test: index === 0 ? 'guarded CDP smoke' : `passing smoke ${index}`,
        verdict: 'pass',
        failed: { candidate: '0/3', baseline: '0/0' },
      })),
      artifact: {
        name: `${side}-canary`,
        url: `https://github.com/fixture/lane/actions/runs/42/artifacts/${side === 'base' ? 1 : 2}`,
      },
    };
  }
  save(path.join(directory, 'plan.json'), plan);
  return { directory, packs, plan, results };
}

function invoke(f, script = 'pr-bios.mjs', args = [], overrides = {}) {
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
      ...inputDirectoryArgs(f, script),
      ...args,
    ],
    {
      encoding: 'utf8',
      env: { ...process.env, CERT_BIOS_ACTUAL_ROWS: '', CERT_PR_PACKS: f.packs, ...overrides },
    }
  );
  const document = read(out);
  assertVerdict(invocation.stdout, script.replace(/\.mjs$/, ''), document.status);
  return { ...invocation, document };
}

const failure = {
  shard: 'guard',
  test: 'guarded CDP smoke',
  log: 'failure',
  failed: { candidate: '3/3', baseline: '0/3' },
};
function failRow(f, side, kind) {
  const result = f.results[side];
  result.counts.pass = 19;
  result.counts[kind] = 1;
  result[{ regression: 'regressions', fail: 'failures', both: 'both', flaky: 'flaky' }[kind]] = [
    failure,
  ];
  result.tests[0] = { ...result.tests[0], verdict: kind, failed: failure.failed };
}

test('without actual-row opt-in the strict baseline-flake classification is byte-for-byte unchanged', (t) => {
  const f = fixture(t);
  failRow(f, 'base', 'flaky');
  const failed = { candidate: '1/3', baseline: '0/3' };
  f.results.base.tests[0].failed = failed;
  f.results.base.flaky = [{ ...failure, failed }];
  f.results.base.tests[0].runs = ['pass', 'fail', 'pass'];
  f.results.candidate.tests[0].runs = ['pass', 'pass', 'pass'];
  const result = invoke(f).document;
  const incomplete = [
    'bios: base: flaky smoke',
    'bios: paired base: unexecuted, partial or flaky test guard/test-0',
  ];
  assert.equal(
    JSON.stringify({
      status: result.status,
      failures: result.failures,
      incomplete: result.incomplete,
      flags: result.flags,
      reason: result.reason,
    }),
    JSON.stringify({
      status: 'INCOMPLETE',
      failures: [],
      incomplete,
      flags: [],
      reason: incomplete.join('; '),
    })
  );
  assert.equal(result.rows, undefined);
  assert.deepEqual(result.advisories, []);
  assert.deepEqual(result.biosLedger, []);
});

for (const mode of ['env missing documents', 'env mismatched documents', 'direct flag accepted'])
  test(`actual-row opt-in never relaxes ${mode}`, async (t) => {
    const f = fixture(t);
    for (const side of ['base', 'candidate'])
      save(path.join(f.directory, `${side}.json`), f.results[side]);
    const server = createServer((request, response) => {
      assert.match(request.url, /^\/repos\/fixture\/lane\/actions\/runs\/42\/artifacts\?/);
      const artifacts =
        mode === 'env mismatched documents'
          ? [
              {
                id: 20,
                name: 'base-canary-shard-canary-packages-update-cdp',
                digest: `sha256:${'d'.repeat(64)}`,
                expired: false,
                workflow_run: { id: 99, head_sha: 'f'.repeat(40) },
              },
            ]
          : [];
      response.setHeader('Content-Type', 'application/json');
      response.end(JSON.stringify({ total_count: artifacts.length, artifacts }));
    });
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
    t.after(() => new Promise((resolve) => server.close(resolve)));
    const out = path.join(f.directory, 'actual-rows-result.json');
    const result = await new Promise((resolve, reject) => {
      const child = spawn(
        process.execPath,
        [
          path.join(root, 'scripts/cert/pr-bios.mjs'),
          '--plan',
          path.join(f.directory, 'plan.json'),
          '--out',
          out,
          ...(mode === 'direct flag accepted' ? ['--rows'] : []),
        ],
        {
          env: {
            ...process.env,
            CERT_PR_PACKS: f.packs,
            CERT_BIOS_ACTUAL_ROWS: mode === 'direct flag accepted' ? '' : '1',
            GH_TOKEN: '',
            GITHUB_API_URL: `http://127.0.0.1:${server.address().port}`,
          },
        }
      );
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
    assert.equal(result.code, 0, result.stderr);
    const value = read(out);
    assert.equal(value.status, 'INCOMPLETE');
    assertVerdict(result.stdout, 'pr-bios', 'INCOMPLETE');
    assert.match(value.reason, /row artifact/);
    assert.notEqual(value.status, 'PASS');
  });

const cases = [
  ['two complete bound calls', () => {}, 'PASS'],
  [
    'shared regressions and both producer rejections are flags',
    (f) => {
      for (const side of ['base', 'candidate']) {
        failRow(f, side, 'regression');
        f.results[side].ok = false;
      }
    },
    'PASS',
  ],
  [
    'failure on bios pins still FAILs if the PR base passes that test',
    (f) => {
      failRow(f, 'candidate', 'both');
    },
    'FAIL',
  ],
  [
    'missing complete tests field is not a compact-count PASS',
    (f) => {
      delete f.results.base.tests;
    },
    'INCOMPLETE',
  ],
  [
    'integrity failure retains precedence with missing base',
    (f) => {
      f.results.base = null;
      f.results.candidate.overrides[0].sha256 = 'f'.repeat(64);
    },
    'FAIL',
  ],
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
      failRow(f, 'candidate', 'regression');
    },
    'FAIL',
  ],
  [
    'candidate failing smoke',
    (f) => {
      failRow(f, 'candidate', 'fail');
    },
    'FAIL',
  ],
  [
    'missing base prevents a candidate-introduced failure proof',
    (f) => {
      f.results.base = null;
      failRow(f, 'candidate', 'regression');
    },
    'INCOMPLETE',
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
      failRow(f, 'candidate', 'both');
      failRow(f, 'base', 'both');
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
    if (name.startsWith('shared regressions'))
      assert.match(result.document.flags.join(';'), /producer ok=false explained/);
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

function boundFixture(t) {
  const f = fixture(t);
  const sha = 'e'.repeat(64);
  f.bindings = {
    schema: 1,
    ...prIdentity(f.plan),
    artifacts: Object.entries(artifactNames(f.plan)).map(([role, name], index) => ({
      role,
      name,
      id: String(index + 1),
      digest: sha,
    })),
  };
  f.actual = f.bindings.artifacts.map((row) => ({
    id: Number(row.id),
    name: row.name,
    digest: `sha256:${sha}`,
    expired: false,
    workflow_run: { id: 42, head_sha: f.plan.head },
  }));
  f.outputs = {};
  for (const side of ['base', 'candidate']) {
    const rows = f.bindings.artifacts.filter((row) => row.role.startsWith(`bios-${side}-`));
    const report = rows.find((row) => row.role.endsWith('-report'));
    f.results[side].biosRef = 'c2cec127b935419049568f584093d7ef03ac0721';
    f.results[side].artifact.url =
      `https://github.com/fixture/lane/actions/runs/42/artifacts/${report.id}`;
    f.outputs[side] = {
      result: f.results[side],
      'artifact-id': report.id,
      'artifact-digest': report.digest,
      artifacts: rows.map((row) => ({
        ...row,
        role: row.role.slice(`bios-${side}-`.length).split('-')[0],
        attempt: '1',
      })),
    };
  }
  return f;
}

async function boundInvoke(t, f, script = 'pr-bios.mjs') {
  const server = createServer((request, response) => {
    assert.match(request.url, /^\/repos\/fixture\/lane\/actions\/runs\/42\/artifacts\?/);
    response.setHeader('Content-Type', 'application/json');
    response.end(JSON.stringify({ total_count: f.actual.length, artifacts: f.actual }));
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise((resolve) => server.close(resolve)));
  const supportsBindings = typeof biosLibrary.selectBiosPin === 'function';
  for (const side of ['base', 'candidate']) {
    const output = supportsBindings ? f.outputs[side] : f.outputs[side]?.result;
    save(path.join(f.directory, `${side}.json`), output);
  }
  save(path.join(f.directory, 'bindings.json'), f.bindings);
  const out = path.join(f.directory, 'bound.json');
  const processResult = await new Promise((resolve, reject) => {
    const child = spawn(
      process.execPath,
      [
        path.join(root, 'scripts/cert', script),
        '--plan',
        path.join(f.directory, 'plan.json'),
        '--out',
        out,
        ...inputDirectoryArgs(f, script),
        ...(supportsBindings ? ['--bindings', path.join(f.directory, 'bindings.json')] : []),
      ],
      {
        env: {
          ...process.env,
          GH_TOKEN: '',
          GITHUB_API_URL: `http://127.0.0.1:${server.address().port}`,
          CERT_PR_PACKS: f.packs,
        },
      }
    );
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (data) => {
      stdout += data;
    });
    child.stderr.on('data', (data) => {
      stderr += data;
    });
    child.on('error', reject);
    child.on('close', (status) => resolve({ status, stdout, stderr }));
  });
  const document = read(out);
  assertVerdict(processResult.stdout, script.replace(/\.mjs$/, ''), document.status);
  return { ...processResult, document };
}

test('parallel bios input planners own their extraction directories and cleanup', async (t) => {
  const fixtures = Array.from({ length: 4 }, () => boundFixture(t));
  const results = await Promise.all(fixtures.map((f) => boundInvoke(t, f, 'pr-bios-inputs.mjs')));
  for (const [index, result] of results.entries()) {
    assert.equal(result.document.status, 'PASS', result.stderr);
    for (const side of ['base', 'candidate']) {
      const file = path.join(
        fixtures[index].directory,
        'unpacked-inputs',
        side,
        'package/package.json'
      );
      assert.ok(existsSync(file), 'input planner must use its case-owned extraction directory');
      const manifest = read(file);
      assert.equal(manifest.version, fixtures[index].results[side].overrides[0].version);
      assert.equal(
        result.document.calls[side]['candidate-sha256'],
        fixtures[index].results[side].overrides[0].sha256
      );
    }
  }
  rmSync(path.join(fixtures[0].directory, 'unpacked-inputs'), { recursive: true });
  for (const f of fixtures.slice(1))
    assert.equal(
      existsSync(path.join(f.directory, 'unpacked-inputs', 'candidate/package/package.json')),
      true
    );
});

for (const corrupt of [false, true])
  test(`default bios input workspace is unique and cleaned on completion: corrupt=${corrupt}`, (t) => {
    const f = fixture(t);
    if (corrupt) writeFileSync(path.join(f.packs, 'candidate/package.tgz'), 'corrupt bytes');
    const out = path.join(f.directory, 'default-inputs.json');
    const run = spawnSync(
      process.execPath,
      [
        path.join(root, 'scripts/cert/pr-bios-inputs.mjs'),
        '--plan',
        path.join(f.directory, 'plan.json'),
        '--out',
        out,
      ],
      {
        encoding: 'utf8',
        env: {
          ...process.env,
          CERT_PR_PACKS: f.packs,
          TMPDIR: f.directory,
          TMP: f.directory,
          TEMP: f.directory,
        },
      }
    );
    assert.equal(read(out).status, corrupt ? 'FAIL' : 'PASS', run.stderr);
    assert.equal(run.status, corrupt ? 1 : 0, run.stderr);
    assert.deepEqual(
      readdirSync(f.directory).filter((entry) => entry.startsWith('slicc-bios-inputs-')),
      []
    );
  });

const bindingCases = [
  [
    'missing callee upload digest',
    (f) => {
      delete f.outputs.candidate.artifacts[0].digest;
    },
    'INCOMPLETE',
  ],
  [
    'missing callee upload ID',
    (f) => {
      delete f.outputs.candidate.artifacts[0].id;
    },
    'INCOMPLETE',
  ],
  [
    'malformed callee upload ID',
    (f) => {
      f.outputs.candidate.artifacts[0].id = 'zero';
    },
    'FAIL',
  ],
  [
    'zero callee attempt is present and wrong',
    (f) => {
      f.outputs.candidate.artifacts[0].attempt = 0;
    },
    'FAIL',
  ],
  [
    'raw string result transport',
    (f) => {
      f.outputs.candidate.result = JSON.stringify(f.outputs.candidate.result);
    },
    'PASS',
  ],
  [
    'malformed result JSON',
    (f) => {
      f.outputs.candidate.result = '{';
    },
    'FAIL',
  ],
  ['all five outputs per label match API', () => {}, 'PASS'],
  [
    'API forged bios suffix',
    (f) => f.actual.push({ ...f.actual[0], id: 90, name: 'candidate-canary-shard-canary-forged' }),
    'FAIL',
  ],
  [
    'API forged evidence suffix',
    (f) => f.actual.push({ ...f.actual[0], id: 90, name: 'cert-pr-evidence-42-1-forged' }),
    'FAIL',
  ],
  [
    'API digest differs from callee upload',
    (f) => {
      f.actual.find((row) => row.name === 'candidate-canary').digest = 'f'.repeat(64);
    },
    'FAIL',
  ],
  [
    'callee manifest digest substitution',
    (f) => {
      f.outputs.candidate.artifacts[0].digest = 'f'.repeat(64);
    },
    'FAIL',
  ],
  [
    'callee manifest ID substitution',
    (f) => {
      f.outputs.candidate.artifacts[0].id = '90';
    },
    'FAIL',
  ],
  [
    'duplicate callee ID',
    (f) => {
      f.outputs.candidate.artifacts[0].id = f.outputs.base.artifacts[0].id;
    },
    'FAIL',
  ],
  [
    'missing callee shard',
    (f) => {
      f.outputs.candidate.artifacts.pop();
    },
    'INCOMPLETE',
  ],
  [
    'unexpected plan upload',
    (f) => {
      f.outputs.candidate.artifacts.push({
        role: 'plan',
        name: 'candidate-plan',
        id: '90',
        digest: 'e'.repeat(64),
        attempt: '1',
      });
    },
    'FAIL',
  ],
  [
    'renamed shard',
    (f) => {
      f.outputs.candidate.artifacts[0].name += '-forged';
    },
    'FAIL',
  ],
  [
    'wrong callee attempt',
    (f) => {
      f.outputs.candidate.artifacts[0].attempt = '2';
    },
    'FAIL',
  ],
  [
    'absent callee attempt',
    (f) => {
      delete f.outputs.candidate.artifacts[0].attempt;
    },
    'INCOMPLETE',
  ],
  [
    'missing direct report ID',
    (f) => {
      delete f.outputs.candidate['artifact-id'];
    },
    'INCOMPLETE',
  ],
  [
    'direct report ID substitution',
    (f) => {
      f.outputs.candidate['artifact-id'] = '90';
    },
    'FAIL',
  ],
  [
    'direct report digest substitution',
    (f) => {
      f.outputs.candidate['artifact-digest'] = 'f'.repeat(64);
    },
    'FAIL',
  ],
  [
    'result URL substitution',
    (f) => {
      f.outputs.candidate.result.artifact.url =
        'https://github.com/fixture/lane/actions/runs/42/artifacts/90';
    },
    'FAIL',
  ],
  [
    'malformed manifest JSON',
    (f) => {
      f.outputs.candidate.artifacts = '{';
    },
    'FAIL',
  ],
  [
    'string manifest transport',
    (f) => {
      f.outputs.candidate.artifacts = JSON.stringify(f.outputs.candidate.artifacts);
    },
    'PASS',
  ],
  [
    'wrong API head',
    (f) => {
      f.actual.find((row) => row.name === 'candidate-canary').workflow_run.head_sha = 'f'.repeat(
        40
      );
    },
    'FAIL',
  ],
  [
    'malformed pack uploader digest',
    (f) => {
      f.bindings.artifacts.find((row) => row.role === 'pack-candidate').digest = 'SHA256:bad';
    },
    'FAIL',
  ],
  [
    'missing pack uploader digest',
    (f) => {
      delete f.bindings.artifacts.find((row) => row.role === 'pack-candidate').digest;
    },
    'INCOMPLETE',
  ],
  [
    'missing manifest and forged API suffix keeps FAIL precedence',
    (f) => {
      delete f.outputs.base.artifacts;
      f.actual.push({ ...f.actual[0], id: 90, name: 'base-canary-files-canary-forged' });
    },
    'FAIL',
  ],
  [
    'wrong pin and forged manifest keeps FAIL precedence',
    (f) => {
      f.outputs.candidate.result.biosRef = pin;
      f.outputs.candidate.artifacts[0].digest = 'f'.repeat(64);
    },
    'FAIL',
  ],
];

for (const [name, mutate, expected] of bindingCases)
  test(`bound bios public CLI: ${name}`, async (t) => {
    const f = boundFixture(t);
    mutate(f);
    const run = await boundInvoke(t, f);
    assert.equal(run.document.status, expected, run.stderr);
    assert.equal(run.status, expected === 'FAIL' ? 1 : 0, run.stderr);
    assert.equal(run.document.pin.commit, 'c2cec127b935419049568f584093d7ef03ac0721');
  });

for (const invalid of [false, true])
  test(`bound bios inputs carry both distinct actual pack uploads: substitution=${invalid}`, async (t) => {
    const f = boundFixture(t);
    if (invalid)
      f.actual.find(
        (row) => row.name === f.bindings.artifacts.find((row) => row.role === 'pack-base').name
      ).digest = 'f'.repeat(64);
    const run = await boundInvoke(t, f, 'pr-bios-inputs.mjs');
    assert.equal(run.document.status, invalid ? 'FAIL' : 'PASS', run.stderr);
    if (invalid) assert.equal(run.document.ready, false);
    else
      for (const side of ['base', 'candidate']) {
        const row = f.bindings.artifacts.find((row) => row.role === `pack-${side}`);
        const call = run.document.calls[side];
        assert.equal(call['candidate-artifact-id'], row.id);
        assert.equal(call['candidate-artifact-digest'], row.digest);
        assert.equal(call['candidate-binding'], 'strict');
        assert.equal(call['bios-ref'], 'c2cec127b935419049568f584093d7ef03ac0721');
        assert.equal(call['candidate-sha256'], f.results[side].overrides[0].sha256);
        assert.notEqual(call['candidate-artifact-digest'], call['candidate-sha256']);
      }
  });

test('strict schema cannot silently downgrade to legacy without binding data', (t) => {
  const f = boundFixture(t);
  assert.equal(
    typeof biosLibrary.selectBiosPin,
    'function',
    'main-owned binding pin selector required'
  );
  const selected = biosLibrary.selectBiosPin(
    read(path.join(root, 'scripts/cert/pr-bios.json')),
    true
  );
  const subjects = Object.fromEntries(
    ['base', 'candidate'].map((side) => [
      side,
      {
        sha256: f.results[side].overrides[0].sha256,
        manifest: { version: f.results[side].overrides[0].version },
      },
    ])
  );
  const findings = biosLibrary.biosFindings(
    { pin: selected, results: f.results, calls: {} },
    selected,
    f.plan,
    subjects
  );
  assert.ok(findings.incomplete.length);
  assert.ok(
    findings.incomplete.some((reason) =>
      /producer binding|required|manifest unavailable/.test(reason)
    )
  );
});
