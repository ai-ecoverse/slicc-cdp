import { mkdirSync, realpathSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import { save } from './common.mjs';
import { observeIdentity } from './execution-identity.mjs';
import { boundedBudget, suitePolicy } from './suite-policy.mjs';
import { executeWorker, guardFile } from './suite-worker.mjs';

export async function suite({
  cwd,
  files,
  out,
  env = {},
  retry = true,
  workerTimeoutMs = suitePolicy.workerTimeoutMs,
  budgetMs = suitePolicy.suiteBudgetMs,
  deadline,
  guardTargetParallel = suitePolicy.guardTargetParallel,
}) {
  if (
    !Number.isSafeInteger(guardTargetParallel) ||
    guardTargetParallel < 1 ||
    guardTargetParallel > suitePolicy.guardTargetParallel
  )
    throw new Error('guard target parallelism must stay within the suite policy');
  cwd = realpathSync(cwd);
  workerTimeoutMs = boundedBudget(workerTimeoutMs, suitePolicy.workerTimeoutMs, 'worker timeout');
  budgetMs = boundedBudget(budgetMs, suitePolicy.suiteBudgetMs, 'suite budget');
  const started = performance.now();
  let executionIdentity;
  if (env.CERT_EXECUTION_IDENTITY) {
    try {
      executionIdentity = observeIdentity(
        cwd,
        JSON.parse(env.CERT_EXECUTION_IDENTITY),
        env.CERT_EXECUTION_SCOPE,
        env.CERT_EXECUTION_TOOLS,
        JSON.parse(env.CERT_EXECUTION_PROOFS ?? '[]'),
        files
      );
      env = { ...env, SLICC_GUARD_SHA: executionIdentity.subject.commit };
    } catch (error) {
      const result = {
        status: error.certStatus ?? 'INCOMPLETE',
        reason: error.message,
        files,
        rows: [],
        attempts: [],
        missing: files,
      };
      save(`${out}.json`, result);
      return result;
    }
  }
  deadline = Math.min(deadline ?? Infinity, started + budgetMs);
  if (!Number.isFinite(deadline)) throw new Error('suite deadline must be finite');
  const policy = {
    ...suitePolicy,
    guardTargetParallel,
    workerTimeoutMs,
    budgetMs,
    isolatedGuardWorker: true,
  };
  const behaviour = (row) =>
    ['test:pass', 'test:fail'].includes(row.type) &&
    !(row.file && row.name && path.resolve(cwd, row.name) === path.resolve(row.file));
  const completed = (worker) =>
    worker.rows.some(behaviour) ||
    worker.rows.some(
      (row) =>
        row.type === 'test:summary' &&
        row.file &&
        path.resolve(row.file) === path.resolve(cwd, worker.file) &&
        row.counts?.tests > 0 &&
        row.counts.cancelled === 0 &&
        row.counts.skipped === 0 &&
        row.counts.todo === 0
    );
  mkdirSync(path.dirname(out), { recursive: true });
  const attempts = [];
  const execute = async (selected) => {
    const index = attempts.length + 1;
    const log = `${out}.${index}.jsonl`;
    const stderr = `${out}.${index}.log`;
    writeFileSync(log, '');
    writeFileSync(stderr, '');
    const workers = [];
    const pending = selected.filter((file) => file !== guardFile);
    const executeFile = async (file) =>
      workers.push(
        await executeWorker({
          cwd,
          file,
          env,
          deadline,
          index,
          out,
          log,
          stderr,
          workerTimeoutMs,
          guardTargetParallel,
        })
      );
    const consume = async () => {
      while (pending.length) await executeFile(pending.shift());
    };
    await Promise.all(Array.from({ length: suitePolicy.fileWorkers }, consume));
    for (const file of selected.filter((file) => file === guardFile)) await executeFile(file);
    const lost = workers.some((worker) => worker.exit === null || worker.signal || worker.timedOut);
    const result = {
      files: selected,
      exit: lost ? null : workers.some((worker) => worker.exit !== 0) ? 1 : 0,
      rows: workers.flatMap((worker) => worker.rows),
      workers,
      policy,
      elapsedMs: Math.round(performance.now() - started),
    };
    attempts.push(result);
    save(`${out}.${index}.json`, result);
    return result;
  };
  const first = await execute(files);
  const repeated = first.workers
    .filter(
      (worker) =>
        !completed(worker) ||
        worker.exit !== 0 ||
        worker.reporterComplete === false ||
        worker.rows.some((row) => row.skip || row.todo || row.type === 'test:fail') ||
        worker.guardReport?.status === 'INCOMPLETE'
    )
    .map((worker) => worker.file);
  const retryBudgetAvailable = performance.now() < deadline;
  if (retry && repeated.length && retryBudgetAvailable) await execute(repeated);
  const latest = attempts.at(-1);
  const workers =
    attempts.length === 1
      ? first.workers
      : [...first.workers.filter((worker) => !repeated.includes(worker.file)), ...latest.workers];
  const rows = workers.flatMap((worker) => worker.rows);
  const missing = workers.filter((worker) => !completed(worker)).map((worker) => worker.file);
  const incomplete =
    missing.length ||
    workers.some(
      (worker) =>
        worker.exit === null ||
        worker.signal ||
        worker.timedOut ||
        worker.reporterComplete === false ||
        worker.guardReport?.status === 'INCOMPLETE'
    ) ||
    rows.some(
      (row) =>
        row.skip ||
        row.todo ||
        ['cancelledByParent', 'testTimeoutFailure'].includes(row.failureType)
    );
  const result = {
    status: incomplete
      ? 'INCOMPLETE'
      : workers.some((worker) => worker.exit !== 0) || rows.some((row) => row.type === 'test:fail')
        ? 'FAIL'
        : 'PASS',
    files,
    missing,
    rows,
    attempts,
    policy,
    elapsedMs: Math.round(performance.now() - started),
    retryNotRunReason:
      retry && repeated.length && !retryBudgetAvailable ? 'suite budget exhausted' : null,
    guardReport: workers.find((worker) => worker.file === guardFile)?.guardReport ?? null,
    ...(executionIdentity ? { identity: executionIdentity } : {}),
  };
  save(`${out}.json`, result);
  return result;
}
