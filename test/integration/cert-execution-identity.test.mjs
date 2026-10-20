import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import {
  corpusIdentity,
  expectedExecutionIdentity,
  installedCorpus,
  observeIdentity,
  selectedHarnessIdentity,
  sourceIdentity,
  stagedIdentity,
  subjectIdentity,
  validateExecutionIdentity,
} from '../../scripts/cert/execution-identity.mjs';

const root = path.resolve(import.meta.dirname, '../..');
const commit = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim();
const source = sourceIdentity(commit);
const corpus = { ...installedCorpus(root), integrity: source.corpus.integrity };

function fixture(t) {
  const directory = mkdtempSync(path.join(tmpdir(), 'slicc-execution-identity-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const archive = path.join(directory, 'source.tar');
  execFileSync('git', ['archive', '-o', archive, commit], { cwd: root });
  execFileSync('tar', ['-xf', archive, '-C', directory]);
  const installed = path.join(directory, 'node_modules/@ai-ecoverse/slicc-browsers');
  mkdirSync(installed, { recursive: true });
  for (const file of ['package.json', 'guard-vectors.json', 'guard-driver.mjs', 'guard-runner.mjs'])
    cpSync(
      path.join(root, 'node_modules/@ai-ecoverse/slicc-browsers', file),
      path.join(installed, file)
    );
  const seed = stagedIdentity(directory, subjectIdentity(commit), source, 'a'.repeat(64));
  return { directory, installed, seed };
}

function findings(execution, expected) {
  const result = { failures: [], incomplete: [] };
  validateExecutionIdentity(execution, expected, 'unit fixture', (status, reason) =>
    result[status === 'FAIL' ? 'failures' : 'incomplete'].push(reason)
  );
  return result;
}

function historicalFixture(t, kind = 'guard') {
  const configured = JSON.parse(readFileSync(path.join(root, 'scripts/cert/base.json'), 'utf8'));
  const golden = JSON.parse(
    readFileSync(new URL('./cert-historical-cargo.json', import.meta.url), 'utf8')
  );
  assert.equal(golden.version, configured.version);
  assert.ok(golden.sourceCommit.startsWith(configured.commit));
  assert.equal(golden.package.devDependencies?.['@ai-ecoverse/slicc-browsers'], undefined);
  assert.equal(golden.lockProjection.corpusEntryAbsent, true);
  const f = fixture(t);
  for (const item of golden.cargo) writeFileSync(path.join(f.directory, item.file), item.manifest);
  const manifest = structuredClone(golden.package);
  const lock = { lockfileVersion: 3, packages: {} };
  if (kind !== 'guard' && kind !== 'guard-worker') {
    rmSync(path.join(f.directory, 'test/integration/guard-runner.test.mjs'));
    writeFileSync(
      path.join(f.directory, 'test/integration/legacy-probe.test.mjs'),
      'import test from "node:test"; test("nonguard legacy identity fixture",()=>{});\n'
    );
  }
  if (kind === 'dependency-only')
    manifest.devDependencies['@ai-ecoverse/slicc-browsers'] = source.corpus.version;
  if (kind === 'lock-only')
    lock.packages['node_modules/@ai-ecoverse/slicc-browsers'] = {
      version: source.corpus.version,
      integrity: source.corpus.integrity,
    };
  if (kind === 'root-lock-only')
    lock.packages[''] = {
      devDependencies: { '@ai-ecoverse/slicc-browsers': source.corpus.version },
    };
  writeFileSync(path.join(f.directory, 'package.json'), JSON.stringify(manifest));
  writeFileSync(path.join(f.directory, 'package-lock.json'), JSON.stringify(lock));
  execFileSync('git', ['init', '-q'], { cwd: f.directory });
  execFileSync('git', ['add', 'crates', 'test', 'package.json', 'package-lock.json'], {
    cwd: f.directory,
  });
  execFileSync(
    'git',
    [
      '-c',
      'user.name=Fixture',
      '-c',
      'user.email=fixture@example.invalid',
      'commit',
      '-qm',
      'Historical Cargo-only subject fixture',
    ],
    { cwd: f.directory }
  );
  const historical = execFileSync('git', ['rev-parse', 'HEAD'], {
    cwd: f.directory,
    encoding: 'utf8',
  }).trim();
  const subject = subjectIdentity(historical, f.directory);
  assert.deepEqual(Object.keys(subject), ['commit', 'cargo']);
  assert.deepEqual(
    subject.cargo.map(({ file, sha256 }) => ({ file, sha256 })),
    golden.cargo.map(({ file, sha256 }) => ({ file, sha256 }))
  );
  return { ...f, historical, subject };
}

test('historical subject needs Cargo only while the selected current harness still requires its corpus lock', (t) => {
  const f = historicalFixture(t);
  const { historical, subject } = f;
  assert.throws(
    () => sourceIdentity(historical, f.directory),
    /execution corpus exact locked package unavailable/
  );
  const seed = stagedIdentity(f.directory, subject, source, 'a'.repeat(64));
  const observed = observeIdentity(f.directory, seed, 'guard', commit);
  const expected = expectedExecutionIdentity(
    subject,
    source,
    source,
    'guard',
    'a'.repeat(64),
    [],
    corpus
  );
  assert.deepEqual(observed, expected);
  assert.equal(observed.subject.commit, historical);
  assert.equal(observed.harness.commit, commit);
  assert.equal(observed.corpus.version, source.corpus.version);
  assert.notEqual(observed.subject.commit, observed.harness.commit);
});

test('historical positive integration positively binds absent corpus and executes only an assigned nonguard worker', (t) => {
  const f = historicalFixture(t, 'historical');
  const harness = selectedHarnessIdentity(
    f.historical,
    'replay',
    'integration-positive',
    f.directory
  );
  assert.equal(
    selectedHarnessIdentity(f.historical, 'release', 'integration-positive', f.directory).corpus,
    null
  );
  for (const [mode, label] of [
    ['pr', 'integration-positive'],
    ['replay', 'guard-positive'],
    ['release', 'guard-negative'],
    ['replay', 'other'],
  ])
    assert.throws(
      () => selectedHarnessIdentity(f.historical, mode, label, f.directory),
      /exact locked package unavailable/
    );
  assert.equal(harness.corpus, null);
  assert.throws(
    () => sourceIdentity(f.historical, f.directory),
    /exact locked package unavailable/
  );
  const seed = stagedIdentity(f.directory, f.subject, harness, 'a'.repeat(64));
  const workers = ['test/integration/legacy-probe.test.mjs'];
  const observed = observeIdentity(
    f.directory,
    seed,
    'integration',
    commit,
    [],
    workers,
    f.directory
  );
  const expected = expectedExecutionIdentity(
    f.subject,
    harness,
    source,
    'integration',
    'a'.repeat(64)
  );
  assert.deepEqual(observed, expected);
  assert.equal(observed.corpus, null);
  assert.deepEqual(findings({ identity: observed, files: workers, attempts: [] }, expected), {
    failures: [],
    incomplete: [],
  });
  const result = execFileSync(process.execPath, ['--test', workers[0]], {
    cwd: f.directory,
    encoding: 'utf8',
    env: Object.fromEntries(
      Object.entries(process.env).filter(
        ([key]) => !/^(CERT_|GITHUB_|NODE_TEST_CONTEXT$)/.test(key)
      )
    ),
  });
  assert.match(result, /nonguard legacy identity fixture/);
  assert.throws(
    () => observeIdentity(f.directory, seed, 'guard', commit, [], workers, f.directory),
    /requires assigned non-guard integration workers/
  );
  assert.throws(
    () =>
      observeIdentity(
        f.directory,
        seed,
        'integration',
        commit,
        [],
        ['test/integration/guard-runner.test.mjs'],
        f.directory
      ),
    /requires assigned non-guard integration workers/
  );
  assert.throws(
    () => expectedExecutionIdentity(f.subject, harness, source, 'guard', 'a'.repeat(64)),
    /guard execution requires exact corpus lock/
  );
  assert.equal(
    findings({ identity: { ...observed, scope: 'guard' }, files: workers }, expected).incomplete
      .length,
    1
  );
  assert.equal(
    findings({ identity: observed, files: ['test/integration/guard-runner.test.mjs'] }, expected)
      .incomplete.length,
    1
  );
  assert.equal(
    findings(
      { identity: observed, files: workers, guardReport: { report: { results: [] } } },
      expected
    ).incomplete.length,
    1
  );
});

for (const kind of ['dependency-only', 'lock-only', 'root-lock-only', 'guard-worker'])
  test(`historical partial or guard-selected corpus ${kind} stays unavailable`, (t) => {
    const f = historicalFixture(t, kind);
    assert.throws(
      () => sourceIdentity(f.historical, f.directory, true),
      /execution corpus exact locked package unavailable/
    );
  });

test('validated staging identifies actual Cargo versions separately from synthetic npm pack versions', (t) => {
  const f = fixture(t);
  const observed = observeIdentity(f.directory, f.seed, 'integration', commit);
  const expected = expectedExecutionIdentity(
    source,
    source,
    source,
    'integration',
    'a'.repeat(64),
    [],
    corpus
  );
  assert.deepEqual(observed, expected);
  assert.deepEqual(findings({ identity: observed, files: [] }, expected), {
    failures: [],
    incomplete: [],
  });
  assert.deepEqual(
    observed.subject.cargo.map((item) => item.version),
    source.cargo.map((item) => item.version)
  );
  assert.equal(observed.subject.commit, commit);
  assert.equal(observed.corpus.version, source.corpus.version);
  assert.equal(observed.harness.overlay, null);
});

for (const [name, file, replacement] of [
  ['harness edit', 'test/integration/guard-client.mjs', '\n'],
  ['Cargo product version edit', 'crates/cdp-client/Cargo.toml', 'version = "99.0.0"'],
  ['driver edit', 'node_modules/@ai-ecoverse/slicc-browsers/guard-driver.mjs', '\n'],
])
  test(`observed ${name} is an integrity FAIL, not metadata PASS`, (t) => {
    const f = fixture(t);
    const target = path.join(f.directory, file);
    const original = readFileSync(target, 'utf8');
    writeFileSync(
      target,
      name.startsWith('Cargo')
        ? original.replace(/^version = "[^"]+"/m, replacement)
        : original + replacement
    );
    assert.throws(
      () => observeIdentity(f.directory, f.seed, 'integration', commit),
      (error) => error.certStatus === 'FAIL'
    );
  });

test('missing Cargo evidence stays unavailable rather than inventing the npm version', (t) => {
  const f = fixture(t);
  rmSync(path.join(f.directory, 'crates/cdp-client/Cargo.toml'));
  assert.throws(
    () => observeIdentity(f.directory, f.seed, 'integration', commit),
    (error) => error.certStatus !== 'FAIL'
  );
});

for (const field of ['commit', 'sha256', 'driverSha256', 'universeSha256'])
  test(`independent record rejects substituted ${field}`, () => {
    const expected = expectedExecutionIdentity(
      source,
      source,
      source,
      'integration',
      'a'.repeat(64),
      [],
      corpus
    );
    const actual = structuredClone(expected);
    if (field === 'commit') actual.subject.commit = 'f'.repeat(40);
    else actual.corpus[field] = 'f'.repeat(64);
    assert.equal(findings({ identity: actual }, expected).failures.length, 1);
  });

for (const missing of [undefined, {}, { schema: 1 }, { schema: 2 }])
  test(`missing or unsupported execution identity stays INCOMPLETE: ${JSON.stringify(missing)}`, () => {
    const expected = expectedExecutionIdentity(
      source,
      source,
      source,
      'integration',
      'a'.repeat(64),
      [],
      corpus
    );
    const value = findings({ identity: missing }, expected);
    assert.equal(value.failures.length, 0);
    assert.equal(value.incomplete.length, 1);
  });

test('exact full keys, not a matching count, bind the guard row universe', () => {
  const expected = expectedExecutionIdentity(
    source,
    source,
    source,
    'integration',
    'a'.repeat(64),
    [],
    corpus
  );
  const vectors = JSON.parse(
    readFileSync(path.join(root, 'node_modules/@ai-ecoverse/slicc-browsers/guard-vectors.json'))
  );
  const results = vectors.vectors.flatMap((vector) =>
    vector.targets.flatMap((target) =>
      Object.keys(vectors.variants).map((variant) => ({
        id: vector.id,
        target,
        variant,
        transport: 'cdp',
      }))
    )
  );
  const report = { sha: commit, set: `@ai-ecoverse/slicc-browsers@${corpus.version}`, results };
  const execution = {
    identity: expected,
    files: ['test/integration/guard-runner.test.mjs'],
    guardReport: { report },
    attempts: [],
  };
  assert.deepEqual(findings(execution, expected), { failures: [], incomplete: [] });
  execution.attempts = [
    { workers: [{ guardReport: { report: { ...report, results: results.slice(1) } } }] },
  ];
  assert.deepEqual(findings(execution, expected), { failures: [], incomplete: [] });
  execution.guardReport.report = { ...report, results: [...results.slice(1), results[1]] };
  assert.equal(findings(execution, expected).incomplete.length, 1);
  execution.guardReport.report = { ...report, sha: 'f'.repeat(40) };
  assert.equal(findings(execution, expected).failures.length, 1);
});

test('every guard shard keeps its independently checked identity and earlier report source', () => {
  const expected = expectedExecutionIdentity(
    source,
    source,
    source,
    'guard',
    'a'.repeat(64),
    [],
    corpus
  );
  const vectors = JSON.parse(
    readFileSync(path.join(root, 'node_modules/@ai-ecoverse/slicc-browsers/guard-vectors.json'))
  );
  const results = vectors.vectors.flatMap((vector) =>
    vector.targets.flatMap((target) =>
      Object.keys(vectors.variants).map((variant) => ({
        id: vector.id,
        target,
        variant,
        transport: 'cdp',
      }))
    )
  );
  const report = { sha: commit, set: `@ai-ecoverse/slicc-browsers@${corpus.version}`, results };
  const execution = {
    identity: expected,
    files: ['test/integration/guard-runner.test.mjs'],
    guardReport: { report },
    attempts: [],
    shards: [{ id: 'a', suite: { identity: structuredClone(expected) } }],
  };
  assert.deepEqual(findings(execution, expected), { failures: [], incomplete: [] });
  delete execution.shards[0].suite.identity;
  assert.equal(findings(execution, expected).incomplete.length, 1);
  execution.shards[0].suite.identity = {
    ...expected,
    subject: { ...expected.subject, commit: 'f'.repeat(40) },
  };
  assert.equal(findings(execution, expected).failures.length, 1);
  execution.shards[0].suite.identity = expected;
  execution.attempts = [
    { workers: [{ guardReport: { report: { ...report, sha: 'f'.repeat(40) } } }] },
  ];
  assert.equal(findings(execution, expected).failures.length, 1);
});

test('the archived subject has no git fallback and the MAIN Rust bridge remains a separate guard harness input', (t) => {
  const f = fixture(t);
  const observed = observeIdentity(f.directory, f.seed, 'guard', commit);
  const expected = expectedExecutionIdentity(
    source,
    source,
    source,
    'guard',
    'a'.repeat(64),
    [],
    corpus
  );
  assert.deepEqual(observed, expected);
  assert.equal(observed.subject.commit, commit);
  assert.throws(() => readFileSync(path.join(f.directory, '.git')), { code: 'ENOENT' });
  const bridge = path.join(f.directory, 'crates/cdp-client/examples/guard_bridge.rs');
  writeFileSync(bridge, readFileSync(bridge) + '\n');
  assert.throws(
    () => observeIdentity(f.directory, f.seed, 'guard', commit),
    (error) => error.certStatus === 'FAIL'
  );
});

test('the named proof overlay binds separately to MAIN rather than relabeling candidate tests', (t) => {
  const f = fixture(t);
  const files = ['test/integration/guard-native.test.mjs'];
  const observed = observeIdentity(f.directory, f.seed, 'proof-integration', commit, files);
  const expected = expectedExecutionIdentity(
    source,
    source,
    source,
    'proof-integration',
    'a'.repeat(64),
    files,
    corpus
  );
  assert.deepEqual(observed, expected);
  assert.equal(observed.harness.commit, commit);
  assert.equal(observed.harness.overlay, commit);
  const helper = path.join(f.directory, 'test/integration/guard-client.mjs');
  writeFileSync(helper, readFileSync(helper) + '\n');
  assert.throws(
    () => observeIdentity(f.directory, f.seed, 'proof-integration', commit, files),
    (error) => error.certStatus === 'FAIL'
  );
});

test('raw corpus bytes bind the installed driver without executing it', (t) => {
  const f = fixture(t);
  assert.deepEqual(corpusIdentity(f.installed), installedCorpus(root));
  writeFileSync(
    path.join(f.installed, 'guard-runner.mjs'),
    'throw new Error("must never execute");\n'
  );
  assert.notEqual(corpusIdentity(f.installed).runnerSha256, corpus.runnerSha256);
});
