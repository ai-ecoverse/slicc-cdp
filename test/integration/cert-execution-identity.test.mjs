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
  sourceIdentity,
  stagedIdentity,
  subjectIdentity,
  validateExecutionIdentity,
} from '../../scripts/cert/execution-identity.mjs';

const root = path.resolve(import.meta.dirname, '../..');
const commit = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim();
const source = sourceIdentity(commit);
const corpus = { ...installedCorpus(root), integrity: source.corpus.integrity };

function fixture(t, subjectCommit = commit) {
  const directory = mkdtempSync(path.join(tmpdir(), 'slicc-execution-identity-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const archive = path.join(directory, 'source.tar');
  execFileSync('git', ['archive', '-o', archive, subjectCommit], { cwd: root });
  execFileSync('tar', ['-xf', archive, '-C', directory]);
  if (subjectCommit !== commit) {
    const harnessArchive = path.join(directory, 'harness.tar');
    execFileSync('git', ['archive', '-o', harnessArchive, commit, 'test'], { cwd: root });
    execFileSync('tar', ['-xf', harnessArchive, '-C', directory]);
  }
  const installed = path.join(directory, 'node_modules/@ai-ecoverse/slicc-browsers');
  mkdirSync(installed, { recursive: true });
  for (const file of ['package.json', 'guard-vectors.json', 'guard-driver.mjs', 'guard-runner.mjs'])
    cpSync(
      path.join(root, 'node_modules/@ai-ecoverse/slicc-browsers', file),
      path.join(installed, file)
    );
  const seed = stagedIdentity(directory, subjectIdentity(subjectCommit), source, 'a'.repeat(64));
  return { directory, installed, seed };
}

function findings(execution, expected) {
  const result = { failures: [], incomplete: [] };
  validateExecutionIdentity(execution, expected, 'unit fixture', (status, reason) =>
    result[status === 'FAIL' ? 'failures' : 'incomplete'].push(reason)
  );
  return result;
}

test('historical subject needs Cargo only while the selected current harness still requires its corpus lock', (t) => {
  const configured = JSON.parse(readFileSync(path.join(root, 'scripts/cert/base.json'), 'utf8'));
  const historical = execFileSync('git', ['rev-parse', `${configured.commit}^{commit}`], {
    cwd: root,
    encoding: 'utf8',
  }).trim();
  const manifest = JSON.parse(
    execFileSync('git', ['show', `${historical}:package.json`], { cwd: root })
  );
  assert.equal(manifest.devDependencies?.['@ai-ecoverse/slicc-browsers'], undefined);
  const subject = subjectIdentity(historical);
  assert.deepEqual(Object.keys(subject), ['commit', 'cargo']);
  assert.equal(subject.cargo.length, 4);
  assert.throws(
    () => sourceIdentity(historical),
    /execution corpus exact locked package unavailable/
  );
  const f = fixture(t, historical);
  const bridge = 'crates/cdp-client/examples/guard_bridge.rs';
  mkdirSync(path.dirname(path.join(f.directory, bridge)), { recursive: true });
  cpSync(path.join(root, bridge), path.join(f.directory, bridge));
  const observed = observeIdentity(f.directory, f.seed, 'guard', commit);
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
