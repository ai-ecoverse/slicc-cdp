import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { fixture } from './cert-fixture.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const vectors = JSON.parse(readFileSync(path.join(root, 'package.json'), 'utf8')).devDependencies[
  '@ai-ecoverse/slicc-browsers'
];

function conclude(documents, extra = []) {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cert-evidence-'));
  try {
    for (const [name, data] of Object.entries(documents))
      writeFileSync(path.join(dir, `${name}.json`), JSON.stringify(data));
    const out = path.join(dir, 'record.json');
    const result = spawnSync(
      process.execPath,
      [path.join(root, 'scripts/cert/conclude.mjs'), '--evidence', dir, '--out', out, ...extra],
      { encoding: 'utf8' }
    );
    assert.ok([0, 1].includes(result.status), result.stderr);
    return { ...result, record: JSON.parse(readFileSync(out, 'utf8')) };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test('cert record CLI accepts complete matching evidence and constructs the candidate line', () => {
  const documents = fixture();
  const result = conclude(documents);
  assert.equal(result.status, 0, result.record.line);
  assert.equal(result.record.certified, true);
  assert.equal(
    result.record.line,
    `certified-candidate @ai-ecoverse/slicc-cdp@1.26.2 sha256=${'a'.repeat(64)} gitHead=${'b'.repeat(40)} guard=GUARD-PASS(slicc-cdp@${'b'.repeat(40)}; vectors @ai-ecoverse/slicc-browsers@${vectors})`
  );
});

test('a declared baseline control regression is a NEGATIVE, not a current guard pass', () => {
  const documents = fixture();
  const row = documents.guard.executions.negative.report.results.find(
    (row) => row.id === 'C1.href'
  );
  row.control = 'fail';
  const key = `${row.id}/${row.target}/${row.variant}`;
  documents.guard.expected = documents.guard.failures = [key];
  documents.plan.proofs.tests.push({
    id: `vector:${key}`,
    source: 'guard-vector',
    vector: row.id,
    target: row.target,
    variant: row.variant,
  });
  documents.guard.executions.negative.status = 'FAIL';
  const result = conclude(documents);
  assert.equal(result.status, 0, result.record.line);
  assert.equal(result.record.certified, true);
});

test('retained guard owner tests prove a historical fix without requesting a deleted integration file', () => {
  const documents = fixture();
  const file = 'test/integration/guard-native.test.mjs';
  const title = 'X1.after-exit seven prompt';
  documents.plan.proofs.tests = [{ id: 'after-exit', file, title, source: 'guard' }];
  documents.integration.fixes = [];
  documents.integration.executions.negative.attempts = [];
  documents.guard.executions.positive.rows.push({ file, name: title, type: 'test:pass' });
  documents.guard.executions.negative.rows.push({
    file,
    name: title,
    type: 'test:fail',
    failureType: 'testCodeFailure',
  });
  const result = conclude(documents);
  assert.equal(result.status, 0, result.record.line);
  documents.guard.executions.negative.rows = [];
  assert.equal(conclude(documents).record.status, 'INCOMPLETE');
});

test('unnamed fixes are known test gaps and unexpected NEGATIVE failures are flags, not missing proof', () => {
  const documents = fixture();
  documents.plan.proofs.known = ['known test gap: unproven fix #173'];
  documents.integration.executions.negative.rows.push({
    type: 'test:fail',
    name: 'unrelated',
    file: 'test/integration/behaviour.test.mjs',
    failureType: 'testCodeFailure',
  });
  const result = conclude(documents);
  assert.equal(result.status, 0, result.record.line);
  assert.ok(result.record.line.includes('unproven fix #173'));
  assert.ok(result.record.line.includes('unexpected NEGATIVE:'));
  documents.plan.proofs.tests = [];
  documents.integration.executions.negative = { rows: [], files: [], attempts: [] };
  assert.equal(conclude(documents).record.status, 'PASS');
});

test('named proof passing on the baseline is INCOMPLETE, not a known-gap waiver', () => {
  const documents = fixture();
  documents.integration.executions.negative.rows[0].type = 'test:pass';
  const result = conclude(documents);
  assert.equal(result.record.status, 'INCOMPLETE');
  assert.ok(result.record.line.includes('proof does not discriminate: fixed-behaviour'));
});

test('record recomputes real ancestry instead of trusting legacyChecksum on a replay plan', () => {
  const history = mkdtempSync(path.join(tmpdir(), 'slicc-checksum-history-'));
  try {
    const git = (...args) => execFileSync('git', args, { cwd: history, encoding: 'utf8' }).trim();
    git('init', '--quiet');
    git('config', 'user.name', 'Fixture');
    git('config', 'user.email', 'fixture@example.invalid');
    const commit = (name) => {
      git('commit', '--quiet', '--allow-empty', '-m', name);
      return git('rev-parse', 'HEAD');
    };
    const before = commit('before checksums');
    const cutoff = commit('complete checksums');
    const after = commit('after checksums');
    const documents = fixture();
    const select = (sha) => {
      documents.plan.commit = sha;
      documents.plan.mode = 'replay';
      documents.plan.legacyChecksum = true;
      for (const [key, value] of Object.entries(documents))
        if (key !== 'plan') {
          value.commit = sha;
          value.mode = 'replay';
          value.legacyChecksum = true;
        }
      documents.registry.gitHead =
        documents.registry.tagCommit =
        documents.registry.provenance.commit =
          sha;
      documents.guard.executions.positive.commit = documents.guard.executions.positive.report.sha =
        sha;
    };
    const installer = documents['assets-linux'].checksums.files.find(
      (file) => file.file === 'install.sh'
    );
    installer.status = 'LEGACY';
    delete installer.expected;
    documents['assets-linux'].checksums.legacy = 'legacy: checksum set unavailable';
    const args = ['--history', history, '--cutoff', cutoff];
    select(before);
    const eligible = conclude(documents, args);
    assert.equal(eligible.record.status, 'PASS', eligible.record.line);
    assert.ok(eligible.record.line.includes('legacy: checksum set unavailable'));
    select(after);
    const ineligible = conclude(documents, args);
    assert.equal(ineligible.record.status, 'INCOMPLETE');
    assert.ok(
      ineligible.record.incomplete.includes(
        'assets-linux: missing complete release checksum evidence'
      )
    );
    assert.equal(
      conclude(documents, ['--history', history, '--cutoff', 'f'.repeat(40)]).record.status,
      'INCOMPLETE'
    );
  } finally {
    rmSync(history, { recursive: true, force: true });
  }
});

test('missing or mismatched required evidence is INCOMPLETE, never a candidate', async (t) => {
  const cases = {
    'missing guard document': (docs) => {
      delete docs.guard;
    },
    'suite file without results': (docs) => {
      docs.integration.executions.positive.rows = [];
    },
    'NEGATIVE never executed': (docs) => {
      docs.integration.executions.negative.attempts = [];
    },
    'skipped required job': (docs) => {
      docs.plan.upstream.guard = 'skipped';
    },
    'different rerun version': (docs) => {
      docs.guard.executions.negative.version = '1.25.2';
    },
    'mixed workflow attempts': (docs) => {
      docs.rebuild.attempt = '2';
    },
    'missing raw guard row despite a pass flag': (docs) => {
      docs.guard.executions.positive.report.results.pop();
    },
    'skipped executable guard row': (docs) => {
      const row = docs.guard.executions.positive.report.results.find((row) => row.id === 'C1.href');
      row.control = row.guarded = row.live = 'skip';
    },
    'missing fresh tag verification': (docs) => {
      delete docs.registry.tagCommit;
    },
    'signature audit did not complete': (docs) => {
      docs.registry.signatures.exit = null;
    },
    'notarisation timed out': (docs) => {
      docs['assets-macos'].files[0].signature.notarisation.exit = null;
    },
    'missing signer identity result': (docs) => {
      delete docs['assets-macos'].files[0].signature.team;
    },
    'missing provenance results': (docs) => {
      delete docs.registry.provenance;
    },
    'missing wasm comparison': (docs) => {
      docs.rebuild.files = [];
    },
    'missing installer checksum': (docs) => {
      docs['assets-linux'].checksums.files = docs['assets-linux'].checksums.files.filter(
        (row) => row.file !== 'install.sh'
      );
    },
    'NEGATIVE lost its Rust test driver rather than exposing the fix': (docs) => {
      docs.integration.executions.negative.rows[0].error = 'running 0 tests';
    },
    'no positive/NEGATIVE finding for a fix': (docs) => {
      docs.integration.executions.proofs.rows = [];
    },
  };
  for (const [name, change] of Object.entries(cases))
    await t.test(name, () => {
      const documents = fixture();
      change(documents);
      const result = conclude(documents);
      assert.equal(result.status, 1);
      assert.equal(result.record.status, 'INCOMPLETE', result.record.line);
      assert.equal(result.record.certified, false);
      assert.match(result.record.line, /^cert-ci: INCOMPLETE/);
    });
});

test('every skipped or cancelled evidence producer is INCOMPLETE and names its job', async (t) => {
  for (const job of ['rebuild', 'guard', 'integration', 'assets']) {
    for (const state of ['skipped', 'cancelled'])
      await t.test(`${job} ${state}`, () => {
        const documents = fixture();
        documents.plan.upstream[job] = state;
        if (job === 'assets') {
          delete documents['assets-linux'];
          delete documents['assets-macos'];
        } else delete documents[job];
        const result = conclude(documents);
        assert.equal(result.record.status, 'INCOMPLETE', result.record.line);
        assert.equal(result.record.certified, false);
        assert.ok(result.record.incomplete.includes(`${job}: upstream ${state}`));
      });
  }
});

test('a failed, skipped or cancelled release caller cannot certify independently passing evidence', async (t) => {
  for (const job of ['release', 'upload-assets', 'verify-signatures']) {
    for (const state of ['failure', 'skipped', 'cancelled'])
      await t.test(`${job} ${state}`, () => {
        const documents = fixture();
        documents.plan.caller = { [job]: state };
        const result = conclude(documents);
        assert.equal(result.record.status, 'INCOMPLETE');
        assert.equal(result.record.certified, false);
        assert.ok(result.record.incomplete.includes(`caller ${job}: ${state}`));
      });
  }
});

test('a byte mismatch or coded supply-chain defect FAILs even with incomplete evidence', async (t) => {
  for (const field of ['registry', 'rebuild', 'tag'])
    await t.test(field, () => {
      const documents = fixture();
      delete documents.integration;
      if (field === 'registry') documents.registry.status = 'FAIL';
      else if (field === 'tag') documents.registry.tagCommit = 'f'.repeat(40);
      else documents.rebuild.sha256 = 'f'.repeat(64);
      const result = conclude(documents);
      assert.equal(result.record.status, 'FAIL');
      assert.equal(result.record.certified, false);
    });
});

test('release asset checker detects installer omission and tampering through its CLI', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cert-assets-'));
  try {
    const inventory = JSON.parse(
      readFileSync(path.join(root, 'scripts/cert/inventory.json'), 'utf8')
    );
    const lines = inventory.map((file) => {
      const bytes = Buffer.from(`asset ${file}`);
      writeFileSync(path.join(dir, file), bytes);
      return `${createHash('sha256').update(bytes).digest('hex')}  ${file}\n`;
    });
    const invoke = () => {
      const result = spawnSync(
        process.execPath,
        [path.join(root, 'scripts/cert/asset-check.mjs'), '--directory', dir],
        { encoding: 'utf8' }
      );
      return JSON.parse(result.stdout);
    };
    writeFileSync(path.join(dir, 'SHA256SUMS'), lines.join(''));
    assert.equal(invoke().status, 'PASS');
    writeFileSync(
      path.join(dir, 'SHA256SUMS'),
      lines.filter((line) => !line.endsWith('install.sh\n')).join('')
    );
    assert.equal(invoke().status, 'INCOMPLETE');
    writeFileSync(path.join(dir, 'SHA256SUMS'), lines.join(''));
    writeFileSync(path.join(dir, 'install.sh'), 'tampered');
    assert.equal(invoke().status, 'FAIL');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('suite CLI reports a file without behaviour results as INCOMPLETE and keeps both attempts', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cert-empty-'));
  try {
    writeFileSync(path.join(dir, 'empty.test.mjs'), 'export const value = 1;\n');
    const output = path.join(dir, 'suite');
    const result = spawnSync(
      process.execPath,
      [
        path.join(root, 'scripts/cert/run-suite.mjs'),
        '--directory',
        dir,
        '--files',
        '["empty.test.mjs"]',
        '--out',
        output,
      ],
      { encoding: 'utf8', timeout: 30000 }
    );
    assert.equal(result.status, 1, result.stdout);
    const report = JSON.parse(readFileSync(`${output}.json`, 'utf8'));
    assert.equal(report.status, 'INCOMPLETE');
    assert.deepEqual(report.missing, ['empty.test.mjs']);
    assert.equal(report.attempts.length, 2);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('suite CLI records real results and retries exactly the same failing file', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cert-suite-'));
  try {
    const source = `import test from 'node:test'; import assert from 'node:assert/strict'; import {existsSync,writeFileSync} from 'node:fs'; test('behaviour',()=>{const seen=existsSync('attempt');writeFileSync('attempt','exact-version');assert.ok(seen);});`;
    writeFileSync(path.join(dir, 'behaviour.test.mjs'), source);
    const output = path.join(dir, 'suite');
    const result = spawnSync(
      process.execPath,
      [
        path.join(root, 'scripts/cert/run-suite.mjs'),
        '--directory',
        dir,
        '--files',
        JSON.stringify(['behaviour.test.mjs']),
        '--out',
        output,
      ],
      { encoding: 'utf8', timeout: 30000 }
    );
    assert.equal(result.status, 0, `${result.stderr}\n${result.stdout}`);
    const report = JSON.parse(readFileSync(`${output}.json`, 'utf8'));
    assert.equal(report.status, 'PASS');
    assert.equal(report.attempts.length, 2);
    assert.equal(report.attempts[0].exit, 1);
    assert.equal(report.attempts[1].exit, 0);
    assert.deepEqual(
      report.attempts.map((attempt) => attempt.files),
      [['behaviour.test.mjs'], ['behaviour.test.mjs']]
    );
    assert.ok(report.rows.some((row) => row.name === 'behaviour' && row.type === 'test:pass'));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
