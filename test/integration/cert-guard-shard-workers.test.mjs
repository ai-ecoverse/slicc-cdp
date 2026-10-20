import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import test from 'node:test';
import { aggregateGuardReports } from '../../scripts/cert/guard-shard-report.mjs';
import { guardShardPlan } from '../../scripts/cert/guard-shards.mjs';
import { guardSuite } from '../../scripts/cert/guard-suite.mjs';
import { validateGuard } from '../../scripts/cert/pr-validate.mjs';
import { suite } from '../../scripts/cert/suite.mjs';
import { suitePolicy } from '../../scripts/cert/suite-policy.mjs';

const file = 'test/integration/guard-runner.test.mjs';
const sha = 'a'.repeat(40);
const targets = [
  'seven',
  'ordinary',
  'chrome-newtab',
  'chrome-passwords',
  'vault',
  'chrome-new-tab-page',
  'chrome-settings',
  'vault-x',
  'own',
  'chrome-search',
  'apex',
  'chrome-extension-page',
  'file-root',
  'network-error',
];
const vectors = {
  schemaVersion: 1,
  setVersion: '0.0.0',
  variants: { prompt: 0, lagging: 1 },
  vectors: targets.map((target) => ({ id: `unit-${target}`, targets: [target] })),
};

function fixture(t) {
  const cwd = mkdtempSync(path.join(tmpdir(), 'cert-sharded-guard-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  return {
    cwd,
    out: path.join(cwd, 'suite'),
    files: [file],
    vectors,
    env: { SLICC_GUARD_SHA: sha },
    deadline: performance.now() + 5000,
  };
}

function completed(options, mutate = () => {}) {
  const selected = JSON.parse(options.env.SLICC_GUARD_TARGETS);
  const results = vectors.vectors
    .filter((vector) => selected.includes(vector.targets[0]))
    .flatMap((vector) =>
      Object.keys(vectors.variants).map((variant) => ({
        id: vector.id,
        target: vector.targets[0],
        variant,
        transport: 'cdp',
        control: 'pass',
        guarded: 'pass',
        live: 'pass',
      }))
    );
  const report = {
    set: '@ai-ecoverse/slicc-browsers@0.0.0',
    sha,
    pass: true,
    results,
    missing: [],
    selection: { targets: selected },
    startupRetries: [],
  };
  const result = {
    status: 'PASS',
    files: options.files,
    missing: [],
    elapsedMs: 10,
    rows: [
      { type: 'test:pass', worker: file, file: 'imported-helper.mjs', name: 'unit guard rows' },
    ],
    attempts: [{ workers: [{ file, guardReport: { status: 'CAPTURED', report } }] }],
    guardReport: { status: 'CAPTURED', report },
  };
  mutate(result, selected);
  return result;
}

test('guard stage partitions every scoped row into four owned workers with unchanged total fanout and deadline', async (t) => {
  const options = fixture(t);
  let active = 0;
  let maximum = 0;
  const calls = [];
  const result = await guardSuite({
    ...options,
    runSuite: async (input) => {
      calls.push(input);
      active++;
      maximum = Math.max(maximum, active);
      await new Promise((resolve) => setTimeout(resolve, 5));
      active--;
      return completed(input);
    },
  });
  assert.equal(result.status, 'PASS');
  assert.equal(calls.length, 4);
  assert.equal(maximum, 2);
  assert.equal(new Set(calls.map((call) => call.out)).size, 4);
  for (const call of calls) {
    assert.deepEqual(call.files, [file]);
    assert.equal(call.deadline, options.deadline);
    assert.equal(call.workerTimeoutMs, 960000);
    assert.equal(call.guardTargetParallel, 2);
  }
  assert.equal(maximum * calls[0].guardTargetParallel, suitePolicy.guardTargetParallel);
  assert.equal(result.guardReport.report.results.length, 28);
  assert.equal(result.shardCoverage.status, 'PASS');
  assert.equal(result.attempts.length, 4);
  assert.deepEqual(result.guardShards.plan.keys, guardShardPlan(vectors).keys);
  assert.ok(result.guardReport.sha256);
  assert.equal(JSON.parse(readFileSync(result.guardReport.file)).results.length, 28);
});

for (const broken of [
  'missing',
  'duplicate',
  'wrong-key',
  'wrong-selection',
  'wrong-source',
  'wrong-corpus',
  'missing-source',
  'malformed-rows',
])
  test(`guard shard aggregate refuses ${broken} rather than an empty or same-count PASS`, async (t) => {
    const result = await guardSuite({
      ...fixture(t),
      runSuite: async (input) =>
        completed(input, (result, selected) => {
          if (!selected.includes('seven')) return;
          const report = result.guardReport.report;
          if (broken === 'missing') {
            result.status = 'INCOMPLETE';
            result.guardReport = null;
          }
          if (broken === 'duplicate') report.results[0] = report.results[1];
          if (broken === 'wrong-key') report.results[0].id = 'not-published';
          if (broken === 'wrong-selection') report.selection.targets = ['vault'];
          if (broken === 'wrong-source') report.sha = 'f'.repeat(40);
          if (broken === 'wrong-corpus') report.set = '@ai-ecoverse/slicc-browsers@9.9.9';
          if (broken === 'missing-source') delete report.sha;
          if (broken === 'malformed-rows') report.results[0] = null;
        }),
    });
    assert.equal(
      result.status,
      ['wrong-selection', 'wrong-source', 'wrong-corpus'].includes(broken) ? 'FAIL' : 'INCOMPLETE'
    );
    assert.equal(result.shards.length, 4);
    assert.ok(result.shards.some((shard) => shard.suite.guardReport?.report));
  });

test('a proven row failure keeps precedence when another shard is killed without a report', async (t) => {
  const result = await guardSuite({
    ...fixture(t),
    runSuite: async (input) =>
      completed(input, (result, selected) => {
        if (selected.includes('seven')) {
          result.status = 'FAIL';
          result.guardReport.report.pass = false;
          result.guardReport.report.results[0].control = 'fail';
        }
        if (selected.includes('vault')) {
          result.status = 'INCOMPLETE';
          result.guardReport = null;
          result.attempts = [
            { workers: [{ timedOut: true, signal: 'SIGKILL', timeoutMs: 960000 }] },
          ];
        }
      }),
  });
  assert.equal(result.status, 'FAIL');
  assert.ok(result.shardCoverage.incomplete.length);
  assert.ok(result.guardReport.report.results.some((row) => row.control === 'fail'));
  assert.ok(
    result.attempts.some((attempt) => attempt.workers.some((worker) => worker.signal === 'SIGKILL'))
  );
});

test('native and named proof workers remain separate from the guard corpus workers', async (t) => {
  const calls = [];
  const result = await guardSuite({
    ...fixture(t),
    files: [file, 'proof.test.mjs'],
    runSuite: async (input) => {
      calls.push(input);
      if (input.files[0] === file) return completed(input);
      return {
        status: 'PASS',
        files: input.files,
        missing: [],
        rows: [{ type: 'test:pass', worker: 'proof.test.mjs', name: 'named proof' }],
        attempts: [{ workers: [{ file: 'proof.test.mjs' }] }],
        elapsedMs: 5,
      };
    },
  });
  assert.deepEqual(calls[0].files, ['proof.test.mjs']);
  assert.equal(result.status, 'PASS');
  assert.equal(result.native.rows[0].name, 'named proof');
  assert.equal(result.attempts.length, 5);
});

test('the independent guard reader reconstructs the shard plan and aggregate rather than trusting producer metadata', async (t) => {
  const execution = await guardSuite({
    ...fixture(t),
    runSuite: async (input) => completed(input),
  });
  execution.report = execution.guardReport.report;
  execution.version = '0.0.0';
  execution.commit = sha;
  const plan = {
    version: '0.0.0',
    commit: sha,
    baseline: { version: '0.0.0', commit: sha },
    run: execution.guardShards.plan.run,
    tools: (await import('node:child_process'))
      .execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' })
      .trim(),
  };
  const findings = (positive) => {
    const messages = [];
    validateGuard(
      { executions: { positive, negative: execution } },
      plan,
      vectors,
      (status, reason) => messages.push({ status, reason })
    );
    return messages;
  };
  assert.deepEqual(findings(execution), []);
  const changed = structuredClone(execution);
  changed.shards[0].suite.guardReport.report.results[0].control = 'fail';
  assert.ok(
    findings(changed).some(
      (entry) => entry.status === 'FAIL' && /aggregate differs/.test(entry.reason)
    )
  );
  const renamed = structuredClone(execution);
  renamed.guardShards.plan.shards[0].targets = ['vault'];
  assert.ok(
    findings(renamed).some(
      (entry) => entry.status === 'INCOMPLETE' && /shard plan contradicts/.test(entry.reason)
    )
  );
  const missing = aggregateGuardReports(
    vectors,
    execution.guardShards.plan,
    execution.shards.slice(1)
  );
  assert.equal(missing.coverage.status, 'INCOMPLETE');
});

test('real bounded shard subprocesses emit owned reports and retain their actual per-attempt bytes', async (t) => {
  const options = fixture(t);
  mkdirSync(path.dirname(path.join(options.cwd, file)), { recursive: true });
  writeFileSync(
    path.join(options.cwd, file),
    `
    import assert from 'node:assert/strict';
    import test from 'node:test';
    import { writeFileSync } from 'node:fs';
    test('synthetic partition evidence', () => {
      const targets = JSON.parse(process.env.SLICC_GUARD_TARGETS);
      assert.ok(targets.length <= 4);
      assert.equal(process.env.SLICC_GUARD_TARGET_PARALLEL, '2');
      assert.equal(process.env.SLICC_GUARD_PARALLEL, '4');
      assert.equal(process.env.SLICC_GUARD_TIMEOUT, '900000');
      const results = targets.flatMap(target => ['prompt', 'lagging'].map(variant => ({
        id: 'unit-' + target, target, variant, transport: 'cdp',
        control: 'pass', guarded: 'pass', live: 'pass'
      })));
      writeFileSync(process.env.SLICC_GUARD_REPORT + '.json', JSON.stringify({
        set: '@ai-ecoverse/slicc-browsers@0.0.0', sha: process.env.SLICC_GUARD_SHA,
        pass: true, results, missing: [], selection: { targets },
        diagnostics: [{ retained: 'original partition diagnostic' }]
      }));
    });
  `
  );
  const result = await guardSuite({ ...options, workerTimeoutMs: 2000 });
  assert.equal(result.status, 'PASS');
  assert.equal(result.shards.length, 4);
  const paths = result.shards.map((shard) => shard.suite.guardReport.file);
  assert.equal(new Set(paths).size, 4);
  for (const shard of result.shards) {
    const worker = shard.suite.attempts[0].workers[0];
    assert.equal(worker.timedOut, false);
    assert.equal(worker.timeoutMs, 2000);
    assert.equal(shard.suite.policy.guardTargetParallel, 2);
    assert.equal(
      JSON.parse(readFileSync(shard.suite.guardReport.file)).diagnostics[0].retained,
      'original partition diagnostic'
    );
  }
});

test('lower per-shard target fanout is bounded without allowing an ambient or caller budget increase', async (t) => {
  const options = fixture(t);
  await assert.rejects(suite({ ...options, guardTargetParallel: 5 }), /suite policy/);
  await assert.rejects(suite({ ...options, guardTargetParallel: 0 }), /suite policy/);
  await assert.rejects(suite({ ...options, guardTargetParallel: 1.5 }), /suite policy/);
  await assert.rejects(guardSuite({ ...options, deadline: Infinity }), /finite deadline/);
  await assert.rejects(guardSuite({ ...options, workerTimeoutMs: 960001 }), /worker timeout/);
});
