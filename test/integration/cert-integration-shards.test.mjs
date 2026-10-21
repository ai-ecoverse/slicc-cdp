import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import test from 'node:test';
import { classifyExecution } from '../../scripts/cert/execution-failure.mjs';
import { aggregateGuardReports } from '../../scripts/cert/guard-shard-report.mjs';
import { guardShardPlan } from '../../scripts/cert/guard-shards.mjs';
import { validateIntegrationShards } from '../../scripts/cert/integration-shards.mjs';
import { integrationSuite } from '../../scripts/cert/integration-suite.mjs';
import { validateSuite } from '../../scripts/cert/pr-validate.mjs';

const guard = 'test/integration/guard-runner.test.mjs';
const commit = 'c'.repeat(40);
const vectors = JSON.parse(
  readFileSync(new URL(import.meta.resolve('@ai-ecoverse/slicc-browsers/guard-vectors.json')))
);

function staged(t) {
  const cwd = mkdtempSync(path.join(tmpdir(), 'integration-shard-contract-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  writeFileSync(path.join(cwd, 'package.json'), '{"name":"staged-fixture"}');
  const corpus = path.join(cwd, 'node_modules/@ai-ecoverse/slicc-browsers');
  mkdirSync(corpus, { recursive: true });
  const selected = { ...vectors, setVersion: '9.7.4' };
  writeFileSync(path.join(corpus, 'guard-vectors.json'), JSON.stringify(selected));
  return {
    cwd,
    files: ['public.test.mjs', guard, 'changed.test.mjs', 'proof.test.mjs'],
    env: { SLICC_GUARD_SHA: commit, CERT_EXECUTION_SCOPE: 'integration' },
    out: path.join(cwd, 'suite'),
    deadline: performance.now() + 10000,
    selected,
  };
}

test('integration chooses staged candidate vectors rather than the MAIN guard corpus and retains all selected workers', async (t) => {
  const options = staged(t);
  let routed;
  const regular = async () => {
    throw new Error('monolithic route forbidden');
  };
  const result = await integrationSuite(options, regular, async (input) => {
    routed = input;
    return { status: 'PASS' };
  });
  assert.equal(result.status, 'PASS');
  assert.equal(routed.vectors.setVersion, '9.7.4');
  assert.deepEqual(routed.files, options.files);
  assert.equal(routed.cwd, options.cwd);
  assert.equal(routed.deadline, options.deadline);
  assert.equal(routed.runSuite, regular);
});

test('integration cannot resolve a missing staged corpus from an enclosing MAIN workspace', async (t) => {
  const parent = path.resolve(import.meta.dirname, '../../.cert/router-fixtures');
  mkdirSync(parent, { recursive: true });
  const cwd = mkdtempSync(path.join(parent, 'staged-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  writeFileSync(path.join(cwd, 'package.json'), '{"name":"missing-staged-corpus"}');
  mkdirSync(path.join(cwd, 'node_modules'));
  const result = await integrationSuite(
    {
      cwd,
      files: [guard],
      out: path.join(cwd, 'result'),
      env: {},
      deadline: performance.now() + 1000,
    },
    async () => {
      throw new Error('escaped execution must not start');
    },
    async () => {
      throw new Error('escaped corpus must not be delegated');
    }
  );
  assert.equal(result.status, 'INCOMPLETE');
  assert.match(result.reason, /outside the staged installation/);
});

test('nonguard integration and historical workers do not require an installed guard corpus', async (t) => {
  const options = staged(t);
  rmSync(path.join(options.cwd, 'node_modules'), { recursive: true });
  options.files = ['historical.test.mjs'];
  const result = await integrationSuite(options, async (input) => {
    assert.equal(input, options);
    return { status: 'PASS' };
  });
  assert.equal(result.status, 'PASS');
});

for (const kind of ['timeout', 'missing report', 'cancelled child'])
  test(`pure ${kind} remains INCOMPLETE, not a candidate regression`, () => {
    const row =
      kind === 'timeout'
        ? {
            type: 'test:fail',
            failureType: 'testTimeoutFailure',
            name: 'published guard set',
            worker: guard,
          }
        : kind === 'cancelled child'
          ? {
              type: 'test:fail',
              failureType: 'cancelledByParent',
              name: 'published guard set',
              worker: guard,
            }
          : { type: 'test:pass', name: 'public behaviour', worker: guard };
    const execution = {
      status: 'INCOMPLETE',
      files: [guard],
      rows: [row],
      attempts: [{}],
      missing: [],
      guardReport: { status: 'INCOMPLETE', reason: 'report absent' },
    };
    const emitted = [];
    validateSuite(execution, 'candidate fixture', (status, reason) =>
      emitted.push({ status, reason })
    );
    assert.ok(emitted.some((item) => item.status === 'INCOMPLETE'));
    assert.equal(
      emitted.some((item) => item.status === 'FAIL'),
      false
    );
    assert.equal(classifyExecution(execution), 'INCOMPLETE');
  });

test('actual assertion and independently identified integrity failures retain FAIL precedence over missing reports', () => {
  const execution = {
    status: 'INCOMPLETE',
    files: [guard],
    rows: [
      { type: 'test:fail', failureType: 'testTimeoutFailure', name: 'budget' },
      { type: 'test:fail', failureType: 'testCodeFailure', name: 'real public assertion' },
    ],
    attempts: [{}],
    guardReport: null,
  };
  assert.equal(classifyExecution(execution), 'FAIL');
  assert.equal(
    classifyExecution({ status: 'FAIL', failureKind: 'integrity', rows: [], files: [guard] }),
    'FAIL'
  );
});

function evidence() {
  const plan = { ...guardShardPlan(vectors), head: commit, run: '17' };
  const shards = plan.shards.map((part) => ({
    id: part.id,
    targets: part.targets,
    head: commit,
    run: plan.run,
    setupTimeoutCount: 0,
    wallTimeMs: 1,
    suite: {
      status: 'PASS',
      elapsedMs: 1,
      guardReport: {
        report: {
          sha: commit,
          set: `@ai-ecoverse/slicc-browsers@${vectors.setVersion}`,
          pass: true,
          missing: [],
          selection: { targets: part.targets },
          results: part.keys.map((key) => {
            const [id, target, variant] = JSON.parse(key);
            return {
              id,
              target,
              variant,
              transport: 'cdp',
              control: 'pass',
              guarded: 'pass',
              live: 'pass',
            };
          }),
        },
      },
    },
  }));
  return {
    files: [guard],
    guardShards: { schema: 2, plan },
    shards,
    guardReport: { report: aggregateGuardReports(vectors, plan, shards).report },
  };
}

test('independent integration reader reconstructs shard coverage and the aggregate from the verified candidate corpus', () => {
  const expected = { harness: { tools: 'a'.repeat(40) }, subject: { commit } };
  const inspect = (execution, catalog = vectors) => {
    const messages = [];
    validateIntegrationShards(
      execution,
      expected,
      catalog,
      '17',
      'candidate',
      (status, reason) => messages.push({ status, reason }),
      true,
      'CURRENT'
    );
    return messages;
  };
  assert.deepEqual(inspect(evidence()), []);
  const partial = evidence();
  partial.shards.pop();
  assert.ok(inspect(partial).some((item) => item.status === 'INCOMPLETE'));
  const hidden = evidence();
  hidden.shards[0].suite.guardReport.report.results[0].control = 'fail';
  assert.ok(
    inspect(hidden).some((item) => item.status === 'FAIL' && /aggregate differs/.test(item.reason))
  );
  const absent = evidence();
  delete absent.guardShards.plan;
  assert.ok(inspect(absent).some((item) => item.status === 'INCOMPLETE'));
});
