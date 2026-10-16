import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const write = (dir, name, value) => {
  const file = path.join(dir, name);
  writeFileSync(file, JSON.stringify(value));
  return file;
};

test('proof planner derives ids from genuine fix commit bodies and PR mappings, and records unnamed fixes as known gaps', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-proof-plan-'));
  try {
    const git = (...args) => execFileSync('git', args, { cwd: dir, encoding: 'utf8' }).trim();
    git('init', '--quiet');
    git('config', 'user.name', 'Fixture');
    git('config', 'user.email', 'fixture@example.invalid');
    const commit = (message) => {
      git('commit', '--quiet', '--allow-empty', '-m', message);
      return git('rev-parse', 'HEAD');
    };
    const base = commit('initial');
    const body = commit('fix: named proof\n\nProves: by-body, vector:C1.href/seven/prompt');
    const mapped = commit('fix: declared PR proof');
    const gap = commit('fix: unnamed proof');
    commit('docs: not a fix');
    const head = git('rev-parse', 'HEAD');
    const pulls = write(dir, 'pulls.json', {
      [body]: [{ number: 10 }],
      [mapped]: [{ number: 201 }],
      [gap]: [{ number: 12 }],
    });
    const mapping = write(dir, 'mapping.json', { 201: ['by-pr'] });
    const registry = write(dir, 'registry.json', {
      'by-body': { file: 'test/integration/a.test.mjs', title: 'a', source: 'integration' },
      'by-pr': { file: 'test/integration/b.test.mjs', title: 'b', source: 'integration' },
    });
    const out = path.join(dir, 'proofs.json');
    const result = spawnSync(
      process.execPath,
      [
        path.join(root, 'scripts/cert/proofs.mjs'),
        '--directory',
        dir,
        '--base',
        base,
        '--head',
        head,
        '--pulls',
        pulls,
        '--mapping',
        mapping,
        '--registry',
        registry,
        '--out',
        out,
      ],
      { encoding: 'utf8' }
    );
    assert.equal(result.status, 0, result.stderr);
    const planned = JSON.parse(readFileSync(out, 'utf8'));
    assert.deepEqual(planned.expected, ['by-body', 'by-pr', 'vector:C1.href/seven/prompt']);
    assert.equal(planned.fixes.length, 3);
    assert.deepEqual(planned.known, ['known test gap: unproven fix #12']);
    assert.equal(planned.tests.find((row) => row.id.startsWith('vector:')).source, 'guard-vector');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('non-executable shared-set descriptors do not make a completed named vector NEGATIVE incomplete', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-vector-proof-'));
  try {
    const id = 'vector:C1.href/seven/prompt';
    const plan = write(dir, 'plan.json', {
      proofs: {
        tests: [
          { id, source: 'guard-vector', vector: 'C1.href', target: 'seven', variant: 'prompt' },
        ],
      },
    });
    const vector = {
      id: 'C1.href',
      target: 'seven',
      variant: 'prompt',
      control: 'pass',
      guarded: 'pass',
      live: 'pass',
    };
    const text = {
      id: 'E1',
      target: 'seven',
      variant: 'prompt',
      control: 'not-run',
      guarded: 'not-run',
      live: 'not-run',
    };
    const positive = write(dir, 'positive.json', {
      attempts: [{}],
      rows: [],
      report: { results: [vector, text] },
    });
    const negative = write(dir, 'negative.json', {
      status: 'FAIL',
      attempts: [{}],
      rows: [],
      report: { results: [{ ...vector, guarded: 'fail' }, text] },
    });
    const out = path.join(dir, 'result.json');
    const child = spawnSync(
      process.execPath,
      [
        path.join(root, 'scripts/cert/check-proofs.mjs'),
        '--source',
        'guard',
        '--plan',
        plan,
        '--positive',
        positive,
        '--negative',
        negative,
        '--out',
        out,
      ],
      { encoding: 'utf8' }
    );
    assert.equal(child.status, 0, child.stderr);
    const result = JSON.parse(readFileSync(out, 'utf8'));
    assert.equal(result.status, 'PASS');
    assert.deepEqual(result.observed, [id]);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('named proof verifier distinguishes an exact NEGATIVE, an unexpected failure, a missing run and a nondiscriminating test', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-proof-check-'));
  try {
    const descriptor = {
      id: 'named',
      file: 'test/integration/a.test.mjs',
      title: 'named',
      source: 'integration',
    };
    const plan = write(dir, 'plan.json', { proofs: { tests: [descriptor] } });
    const row = (type, name = 'named') => ({
      type,
      name,
      file: descriptor.file,
      failureType: type === 'test:fail' ? 'testCodeFailure' : undefined,
    });
    const suite = (rows) => ({
      rows,
      attempts: [{ rows, exit: rows.some((row) => row.type === 'test:fail') ? 1 : 0 }],
    });
    const positive = write(dir, 'positive.json', suite([row('test:pass')]));
    const negative = path.join(dir, 'negative.json');
    const out = path.join(dir, 'result.json');
    const check = (value) => {
      writeFileSync(negative, JSON.stringify(value));
      const child = spawnSync(
        process.execPath,
        [
          path.join(root, 'scripts/cert/check-proofs.mjs'),
          '--plan',
          plan,
          '--positive',
          positive,
          '--negative',
          negative,
          '--out',
          out,
        ],
        { encoding: 'utf8' }
      );
      assert.equal(child.status, 0, child.stderr);
      return JSON.parse(readFileSync(out, 'utf8'));
    };
    const interrupted = check({ ...suite([row('test:fail')]), status: 'INCOMPLETE' });
    assert.equal(interrupted.status, 'INCOMPLETE');
    assert.ok(interrupted.incomplete.includes('named proof NEGATIVE worker did not complete'));
    const skipped = check(
      suite([
        row('test:fail'),
        { type: 'test:pass', name: 'sibling', file: descriptor.file, skip: true },
      ])
    );
    assert.equal(skipped.status, 'INCOMPLETE');
    writeFileSync(positive, JSON.stringify({ ...suite([row('test:pass')]), status: 'INCOMPLETE' }));
    const currentLost = check(suite([row('test:fail')]));
    assert.equal(currentLost.status, 'INCOMPLETE');
    assert.ok(currentLost.incomplete.includes('named proof candidate worker did not complete'));
    writeFileSync(
      positive,
      JSON.stringify(
        suite([
          row('test:pass'),
          { type: 'test:pass', name: 'sibling', file: descriptor.file, todo: true },
        ])
      )
    );
    assert.equal(check(suite([row('test:fail')])).status, 'INCOMPLETE');
    writeFileSync(positive, JSON.stringify({ ...suite([row('test:pass')]), status: 'FAIL' }));
    assert.equal(check(suite([row('test:fail')])).status, 'FAIL');
    writeFileSync(
      positive,
      JSON.stringify(suite([row('test:pass'), row('test:fail', 'failed sibling')]))
    );
    assert.equal(check(suite([row('test:fail')])).status, 'FAIL');
    writeFileSync(positive, JSON.stringify(suite([row('test:pass')])));
    const exact = check(suite([row('test:fail')]));
    assert.equal(exact.status, 'PASS');
    assert.deepEqual(exact.expected, exact.observed);
    const extra = check(suite([row('test:fail'), row('test:fail', 'extra')]));
    assert.equal(extra.status, 'PASS');
    assert.equal(extra.unexpected.length, 1);
    const oldPass = check(suite([row('test:pass')]));
    assert.equal(oldPass.status, 'INCOMPLETE');
    assert.ok(oldPass.incomplete.includes('proof does not discriminate: named'));
    const missing = check({ rows: [], attempts: [] });
    assert.equal(missing.status, 'INCOMPLETE');
    assert.ok(missing.incomplete.includes('named proof NEGATIVE did not run'));
    writeFileSync(plan, JSON.stringify({ proofs: { tests: [] } }));
    assert.equal(check({ rows: [], attempts: [] }).status, 'PASS');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
