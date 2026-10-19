import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));

function execute(dir, file, { args = [], env = {}, files = [file] } = {}) {
  const out = path.join(dir, 'result');
  const child = spawnSync(
    'node',
    [
      path.join(root, 'scripts/cert/run-suite.mjs'),
      '--directory',
      dir,
      '--files',
      JSON.stringify(files),
      '--out',
      out,
      ...args,
    ],
    {
      encoding: 'utf8',
      timeout: 30000,
      env: {
        ...Object.fromEntries(
          Object.entries(process.env).filter(([key]) => !/^(CERT_|GITHUB_)/.test(key))
        ),
        GH_TOKEN: '',
        ...env,
      },
    }
  );
  return { process: child, result: JSON.parse(readFileSync(`${out}.json`, 'utf8')), out };
}

test('cert suite counts an entry wrapper whose behavioural tests are defined in an imported helper', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cert-wrapper-'));
  try {
    writeFileSync(path.join(dir, 'entry.test.mjs'), 'import "./helper.mjs";\n');
    writeFileSync(
      path.join(dir, 'helper.mjs'),
      'import test from "node:test"; test("public behaviour",()=>{});\n'
    );
    const { process, result } = execute(dir, 'entry.test.mjs');
    assert.equal(process.status, 0, process.stderr);
    assert.equal(result.status, 'PASS');
    assert.deepEqual(result.missing, []);
    assert.equal(result.attempts.length, 1);
    assert.ok(
      result.rows.some((row) => row.file?.endsWith('helper.mjs') && row.type === 'test:pass')
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('a failed imported-helper worker is retried and its stale helper rows are replaced', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cert-helper-retry-'));
  try {
    writeFileSync(path.join(dir, 'entry.test.mjs'), 'import "./helper.mjs";\n');
    writeFileSync(
      path.join(dir, 'helper.mjs'),
      'import test from "node:test"; import assert from "node:assert/strict"; import {existsSync,writeFileSync} from "node:fs"; test("imported behaviour",()=>{const seen=existsSync("attempt");writeFileSync("attempt","same-version");assert.ok(seen);});\n'
    );
    const { process, result } = execute(dir, 'entry.test.mjs');
    assert.equal(process.status, 0, process.stderr);
    assert.equal(result.attempts.length, 2);
    assert.equal(result.attempts[0].exit, 1);
    assert.equal(result.attempts[1].exit, 0);
    assert.ok(
      result.attempts[0].rows.some(
        (row) => row.type === 'test:fail' && row.file?.endsWith('helper.mjs')
      )
    );
    assert.equal(
      result.rows.some((row) => row.type === 'test:fail'),
      false
    );
    assert.ok(
      result.rows.some((row) => row.worker === 'entry.test.mjs' && row.type === 'test:pass')
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('cert suite keeps a module-load failure diagnostic instead of an empty stderr log', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cert-diagnostic-'));
  try {
    writeFileSync(path.join(dir, 'entry.test.mjs'), 'import "./missing.mjs";\n');
    const { process, result, out } = execute(dir, 'entry.test.mjs');
    assert.equal(process.status, 1);
    assert.equal(result.status, 'INCOMPLETE');
    assert.match(readFileSync(`${out}.1.log`, 'utf8'), /ERR_MODULE_NOT_FOUND/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('the current browser fixture loads independently of an older subject without the browser API', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cert-fixture-'));
  try {
    const helpers = path.join(dir, 'test/integration');
    mkdirSync(helpers, { recursive: true });
    const file = path.join(helpers, 'browser.mjs');
    copyFileSync(path.join(root, 'test/integration/browser.mjs'), file);
    const outcome = spawnSync(
      'node',
      [
        '--input-type=module',
        '-e',
        `const fixture = await import(${JSON.stringify(pathToFileURL(file).href)}); console.log(typeof fixture.launchBrowser);`,
      ],
      {
        encoding: 'utf8',
        env: {
          ...process.env,
          SLICC_FIXTURE_BROWSER_API: pathToFileURL(path.join(root, 'src/browser.js')).href,
        },
      }
    );
    assert.equal(outcome.status, 0, outcome.stderr);
    assert.equal(outcome.stdout.trim(), 'function');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('workflow-call proof CLI requires execution despite a skipped caller ancestor and rejects missing jobs', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cert-call-'));
  try {
    const record = path.join(dir, 'record.json');
    const jobs = path.join(dir, 'jobs.json');
    writeFileSync(
      record,
      JSON.stringify({
        run: '123',
        version: '1.26.2',
        status: 'INCOMPLETE',
        certified: false,
        caller: { 'upload-assets': 'skipped' },
      })
    );
    const required = [
      'plan',
      'rebuild',
      'guard',
      'integration',
      'assets (ubuntu-latest)',
      'assets (macos-latest)',
      'record',
    ];
    const data = {
      jobs: required.map((name) => ({
        name: `cert / ${name}`,
        status: 'completed',
        conclusion: 'success',
      })),
    };
    const run = () =>
      spawnSync(
        'node',
        [path.join(root, 'scripts/cert/verify-call.mjs'), '--record', record, '--jobs', jobs],
        {
          encoding: 'utf8',
          env: { ...process.env, GITHUB_RUN_ID: '123', UPSTREAM: 'skipped' },
        }
      );
    writeFileSync(jobs, JSON.stringify(data));
    assert.equal(run().status, 0);
    for (const state of ['skipped', 'cancelled']) {
      data.jobs[1].conclusion = state;
      writeFileSync(jobs, JSON.stringify(data));
      const rejected = run();
      assert.equal(rejected.status, 1);
      assert.match(rejected.stderr, /required job: rebuild/);
    }
    data.jobs.splice(1, 1);
    writeFileSync(jobs, JSON.stringify(data));
    assert.equal(run().status, 1);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('cert suite preserves nested AggregateError causes in actual failed worker rows', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cert-causes-'));
  try {
    writeFileSync(
      path.join(dir, 'entry.test.mjs'),
      'import test from "node:test"; test("nested CDP failure",()=>{throw new AggregateError([new Error("CDP attach timeout",{cause:new Error("socket lost")})],"target invocations failed",{cause:new Error("setup failed")});});\n'
    );
    const { process, result } = execute(dir, 'entry.test.mjs');
    assert.equal(process.status, 1);
    assert.equal(result.status, 'FAIL');
    assert.equal(result.attempts.length, 2);
    for (const attempt of result.attempts) {
      const row = attempt.rows.find((row) => row.type === 'test:fail');
      assert.equal(row.error, 'target invocations failed');
      const aggregate = row.errorDetails?.cause;
      assert.equal(aggregate?.name, 'AggregateError');
      assert.equal(aggregate.cause.message, 'setup failed');
      assert.equal(aggregate.errors[0].message, 'CDP attach timeout');
      assert.equal(aggregate.errors[0].cause.message, 'socket lost');
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('cert suite records bounded guard fanout and overrides ambient guard settings', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cert-fanout-'));
  try {
    writeFileSync(
      path.join(dir, 'entry.test.mjs'),
      'import test from "node:test"; import {writeFileSync} from "node:fs"; test("actual environment",()=>writeFileSync("environment.json",JSON.stringify({targets:process.env.SLICC_GUARD_TARGET_PARALLEL,slots:process.env.SLICC_GUARD_PARALLEL})));\n'
    );
    const { process, result } = execute(dir, 'entry.test.mjs', {
      env: { SLICC_GUARD_TARGET_PARALLEL: '99', SLICC_GUARD_PARALLEL: '99' },
    });
    assert.equal(process.status, 0, process.stderr);
    assert.deepEqual(JSON.parse(readFileSync(path.join(dir, 'environment.json'))), {
      targets: '4',
      slots: '4',
    });
    assert.equal(result.policy.guardTargetParallel, 4);
    assert.equal(result.policy.guardSlotParallel, 4);
    assert.equal(result.policy.testTimeoutMs, 900000);
    assert.equal(result.policy.workerTimeoutMs, 960000);
    assert.equal(result.policy.suiteBudgetMs + result.policy.setupAndUploadReserveMs, 5400000);
    assert.equal(result.attempts[0].workers[0].timedOut, false);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('cert suite refuses an exhausted enclosing job budget before starting candidate tests', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cert-job-budget-'));
  try {
    writeFileSync(
      path.join(dir, 'entry.test.mjs'),
      'import test from "node:test"; import {writeFileSync} from "node:fs"; test("must not start",()=>writeFileSync("started","yes"));\n'
    );
    const { process, result } = execute(dir, 'entry.test.mjs', {
      env: { CERT_SUITE_JOB_STARTED_MS: String(Date.now() - 5400000) },
    });
    assert.equal(process.status, 1);
    assert.equal(result.status, 'INCOMPLETE');
    assert.equal(result.budget.reason, 'cert job execution budget exhausted');
    assert.equal(result.attempts.length, 1);
    assert.equal(result.attempts[0].workers[0].started, false);
    assert.equal(result.retryNotRunReason, 'suite budget exhausted');
    assert.deepEqual(result.missing, ['entry.test.mjs']);
    assert.throws(() => readFileSync(path.join(dir, 'started')), { code: 'ENOENT' });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('cert suite retains actual worker kills and both planned timeout attempts', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cert-worker-budget-'));
  try {
    writeFileSync(
      path.join(dir, 'entry.test.mjs'),
      'import test from "node:test"; import {setTimeout} from "node:timers/promises"; test("wait for worker budget",async()=>{if(process.env.SLICC_GUARD_TARGET_PARALLEL==="4")await setTimeout(5000);});\n'
    );
    const current = readFileSync(path.join(root, 'scripts/cert/run-suite.mjs'), 'utf8');
    const args = current.includes("'worker-timeout-ms'")
      ? ['--worker-timeout-ms', '400', '--budget-ms', '3000']
      : [];
    const { process, result, out } = execute(dir, 'entry.test.mjs', {
      args,
      env: { SLICC_GUARD_TARGET_PARALLEL: '99' },
    });
    assert.equal(process.status, 1);
    assert.equal(result.status, 'INCOMPLETE');
    assert.equal(result.attempts.length, 2);
    for (const [index, attempt] of result.attempts.entries()) {
      const worker = attempt.workers[0];
      assert.equal(worker.exit, null);
      assert.equal(worker.signal, 'SIGKILL');
      assert.equal(worker.timedOut, true);
      assert.equal(worker.timeoutMs, 400);
      assert.equal(worker.timeoutReason, 'worker budget exhausted');
      assert.match(
        readFileSync(out + '.' + (index + 1) + '.log', 'utf8'),
        /worker budget exhausted/
      );
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('cert suite captures every guard row report attempt and keeps diagnostics as original JSON', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cert-row-reports-'));
  try {
    const file = 'test/integration/guard-runner.test.mjs';
    mkdirSync(path.dirname(path.join(dir, file)), { recursive: true });
    writeFileSync(
      path.join(dir, file),
      'import test from "node:test"; import assert from "node:assert/strict"; import {existsSync,writeFileSync} from "node:fs"; test("complete fake row report",()=>{const pass=existsSync("attempt");writeFileSync("attempt","yes");writeFileSync((process.env.SLICC_GUARD_REPORT??"old-report.md")+".json",JSON.stringify({pass,results:[{id:"test-only-row",control:pass?"pass":"fail"}],missing:[],diagnostics:[{value:"complete original diagnostic"}]}));assert.ok(pass);});\n'
    );
    const { process, result } = execute(dir, file);
    assert.equal(process.status, 0, process.stderr);
    assert.equal(result.status, 'PASS');
    assert.equal(result.attempts.length, 2);
    const reports = result.attempts.map((attempt) => attempt.workers[0].guardReport);
    assert.equal(reports[0]?.status, 'CAPTURED');
    assert.equal(reports[0].report.results[0].control, 'fail');
    assert.equal(reports[1].report.results[0].control, 'pass');
    assert.notEqual(reports[0].file, reports[1].file);
    assert.equal(result.guardReport.file, reports[1].file);
    for (const report of reports) {
      const bytes = readFileSync(report.file);
      assert.equal(bytes.length, report.bytes);
      assert.equal(report.report.diagnostics, undefined);
      assert.equal(report.diagnostics.embedded, false);
      assert.equal(report.diagnostics.count, 1);
      assert.equal(JSON.parse(bytes).diagnostics[0].value, 'complete original diagnostic');
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('cert suite does not reuse an ambient stale report when the guard worker emits none', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cert-stale-report-'));
  try {
    const file = 'test/integration/guard-runner.test.mjs';
    mkdirSync(path.dirname(path.join(dir, file)), { recursive: true });
    writeFileSync(
      path.join(dir, file),
      'import test from "node:test"; test("no report",()=>{});\n'
    );
    const stale = path.join(dir, 'stale.md');
    writeFileSync(stale + '.json', JSON.stringify({ pass: true, results: [{ id: 'stale' }] }));
    const { process, result } = execute(dir, file, { env: { SLICC_GUARD_REPORT: stale } });
    assert.equal(process.status, 1);
    assert.equal(result.status, 'INCOMPLETE');
    assert.equal(result.attempts.length, 2);
    for (const attempt of result.attempts) {
      assert.equal(attempt.workers[0].exit, 0);
      assert.equal(attempt.workers[0].guardReport.status, 'INCOMPLETE');
      assert.notEqual(attempt.workers[0].guardReport.file, stale + '.json');
      assert.equal(attempt.workers[0].guardReport.report, undefined);
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

async function shardTools() {
  const file = path.join(root, 'scripts/cert/guard-shards.mjs');
  assert.ok(existsSync(file), 'fixed guard shard tools are available');
  return import(pathToFileURL(file).href);
}

function shardFixture(guardShardPlan) {
  const targets = [
    'seven',
    'vault',
    'vault-x',
    'apex',
    'own',
    'chrome-newtab',
    'chrome-new-tab-page',
    'chrome-settings',
    'chrome-passwords',
    'chrome-extension-page',
    'file-root',
    'network-error',
    'chrome-search',
  ];
  const plan = {
    ...guardShardPlan({
      vectors: targets.map((target) => ({ id: 'test-only-' + target, targets: [target] })),
      variants: { prompt: 0, lagging: 1 },
    }),
    head: 'test-only-subject',
    run: 'test-only-run',
  };
  const documents = plan.shards.map((shard) => ({
    id: shard.id,
    targets: shard.targets,
    head: plan.head,
    run: plan.run,
    wallTimeMs: 100,
    setupTimeoutCount: 0,
    suite: {
      status: 'PASS',
      elapsedMs: 90,
      guardReport: {
        report: {
          pass: true,
          missing: [],
          selection: { targets: shard.targets },
          results: shard.keys.map((key) => {
            const [id, target, variant] = JSON.parse(key);
            return { id, target, variant, control: 'pass', guarded: 'pass', live: 'pass' };
          }),
        },
      },
    },
  }));
  return { plan, documents };
}

test('guard shard planner partitions all published targets and row keys without overlap', async () => {
  const { guardShardPlan } = await shardTools();
  const { plan } = shardFixture(guardShardPlan);
  assert.deepEqual(
    plan.shards.map((shard) => shard.targets.length),
    [3, 3, 3, 4]
  );
  const keys = plan.shards.flatMap((shard) => shard.keys).sort();
  assert.deepEqual(keys, plan.keys);
  assert.equal(new Set(keys).size, keys.length);
  assert.deepEqual(
    plan.shards.map((shard) => shard.expectedRows),
    [6, 6, 6, 8]
  );
  assert.throws(
    () =>
      guardShardPlan({
        vectors: [{ id: 'test-only', targets: ['unknown'] }],
        variants: { prompt: 0 },
      }),
    /exactly one fixed shard/
  );
});

test('guard shard record requires every declared job and each exact row set, not counts', async () => {
  const { guardShardPlan, recordGuardShards } = await shardTools();
  const { plan, documents } = shardFixture(guardShardPlan);
  assert.equal(recordGuardShards(plan, documents).status, 'PASS');
  const missing = recordGuardShards(plan, documents.slice(1));
  assert.equal(missing.status, 'INCOMPLETE');
  assert.match(missing.incomplete.join(';'), /shard a missing/);
  const sameCountWrongRows = structuredClone(documents);
  sameCountWrongRows[0].suite.guardReport.report.results[0].id = 'wrong-key';
  assert.equal(recordGuardShards(plan, sameCountWrongRows).status, 'INCOMPLETE');
  const duplicate = recordGuardShards(plan, [...documents, documents[0]]);
  assert.equal(duplicate.status, 'FAIL');
  assert.match(duplicate.failures.join(';'), /duplicate/);
});

test('guard shard record keeps killed or unsupported execution INCOMPLETE and positive FAIL red', async () => {
  const { guardShardPlan, recordGuardShards } = await shardTools();
  const { plan, documents } = shardFixture(guardShardPlan);
  const lost = structuredClone(documents);
  lost[0].suite = { status: 'INCOMPLETE', guardReport: null };
  lost[0].setupTimeoutCount = null;
  assert.equal(recordGuardShards(plan, lost).status, 'INCOMPLETE');
  const failed = structuredClone(documents);
  failed[0].suite.status = 'FAIL';
  failed[0].suite.guardReport.report.pass = false;
  assert.equal(recordGuardShards(plan, failed).status, 'FAIL');
  failed[1].suite = lost[0].suite;
  const mixed = recordGuardShards(plan, failed);
  assert.equal(mixed.status, 'FAIL');
  assert.ok(mixed.incomplete.length > 0);
});

test('guard shard record binds target assignment and preserves each job measurement', async () => {
  const { guardShardPlan, recordGuardShards } = await shardTools();
  const { plan, documents } = shardFixture(guardShardPlan);
  const record = recordGuardShards(plan, documents);
  assert.equal(record.measurementOnly, true);
  assert.equal(record.metrics.length, 4);
  assert.equal(record.metrics[0].wallTimeMs, 100);
  assert.equal(record.metrics[0].suiteTimeMs, 90);
  assert.equal(record.metrics[0].setupTimeoutCount, 0);
  const wrong = structuredClone(documents);
  wrong[0].suite.guardReport.report.selection.targets = ['vault'];
  assert.equal(recordGuardShards(plan, wrong).status, 'FAIL');
  wrong[0] = structuredClone(documents[0]);
  wrong[0].head = 'other-subject';
  assert.equal(recordGuardShards(plan, wrong).status, 'FAIL');
});
