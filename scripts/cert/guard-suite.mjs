import { readFileSync } from 'node:fs';
import { performance } from 'node:perf_hooks';
import { save } from './common.mjs';
import { aggregateGuardReports } from './guard-shard-report.mjs';
import { guardSetupTimeoutCount, guardShardPlan } from './guard-shards.mjs';
import { suite } from './suite.mjs';
import { boundedBudget, suitePolicy } from './suite-policy.mjs';
import { captureGuardReport } from './suite-report.mjs';
import { guardFile } from './suite-worker.mjs';

function guardResult({ vectors, shardPlan, shards, native, files, out, started, workerTimeoutMs }) {
  const { coverage, report } = aggregateGuardReports(vectors, shardPlan, shards);
  const executions = [...(native ? [native] : []), ...shards.map((shard) => shard.suite)];
  const guardReport = report
    ? (() => {
        const file = `${out}.guard.md.json`;
        save(file, report);
        return captureGuardReport(file);
      })()
    : null;
  return {
    status:
      coverage.status === 'FAIL' || native?.status === 'FAIL'
        ? 'FAIL'
        : coverage.status === 'INCOMPLETE' || native?.status === 'INCOMPLETE'
          ? 'INCOMPLETE'
          : 'PASS',
    files,
    missing: [...new Set(executions.flatMap((execution) => execution.missing ?? []))],
    rows: executions.flatMap((execution) => execution.rows ?? []),
    attempts: executions.flatMap((execution) => execution.attempts ?? []),
    guardReport,
    shardPlan,
    shards,
    shardCoverage: coverage,
    native,
    elapsedMs: Math.round(performance.now() - started),
    policy: {
      ...suitePolicy,
      workerTimeoutMs,
      guardShardWorkers: suitePolicy.fileWorkers,
      guardTargetsPerWorker: Math.floor(suitePolicy.guardTargetParallel / suitePolicy.fileWorkers),
    },
    ...(executions[0]?.identity ? { identity: executions[0].identity } : {}),
  };
}

export async function guardSuite({
  cwd,
  files,
  out,
  env,
  deadline,
  vectors = JSON.parse(
    readFileSync(new URL(import.meta.resolve('@ai-ecoverse/slicc-browsers/guard-vectors.json')))
  ),
  runSuite = suite,
  workerTimeoutMs = suitePolicy.workerTimeoutMs,
}) {
  const started = performance.now();
  workerTimeoutMs = boundedBudget(workerTimeoutMs, suitePolicy.workerTimeoutMs, 'worker timeout');
  if (!Number.isFinite(deadline)) throw new Error('guard suite requires a shared finite deadline');
  const shardPlan = {
    ...guardShardPlan(vectors),
    head: env.SLICC_GUARD_SHA,
    run: process.env.GITHUB_RUN_ID ?? 'local',
  };
  const execute = async (options) => {
    try {
      return await runSuite(options);
    } catch (error) {
      const result = {
        status: error.certStatus ?? 'INCOMPLETE',
        reason: error.stack,
        files: options.files,
        missing: options.files,
        rows: [],
        attempts: [],
        elapsedMs: 0,
      };
      save(`${options.out}.json`, result);
      return result;
    }
  };
  const nativeFiles = files.filter((file) => file !== guardFile);
  const native = nativeFiles.length
    ? await execute({
        cwd,
        files: nativeFiles,
        out: `${out}.native`,
        env,
        deadline,
        workerTimeoutMs,
      })
    : null;
  const pending = [...shardPlan.shards];
  const shards = [];
  const consume = async () => {
    while (pending.length) {
      const shard = pending.shift();
      const result = await execute({
        cwd,
        files: [guardFile],
        out: `${out}.shard-${shard.id}`,
        env: { ...env, SLICC_GUARD_TARGETS: JSON.stringify(shard.targets) },
        deadline,
        workerTimeoutMs,
        guardTargetParallel: Math.floor(suitePolicy.guardTargetParallel / suitePolicy.fileWorkers),
      });
      shards.push({
        id: shard.id,
        targets: shard.targets,
        head: shardPlan.head,
        run: shardPlan.run,
        wallTimeMs: result.elapsedMs,
        setupTimeoutCount: guardSetupTimeoutCount(result.guardReport?.report),
        suite: result,
      });
    }
  };
  await Promise.all(Array.from({ length: suitePolicy.fileWorkers }, consume));
  shards.sort((a, b) => a.id.localeCompare(b.id));
  const result = guardResult({
    vectors,
    shardPlan,
    shards,
    native,
    files,
    out,
    started,
    workerTimeoutMs,
  });
  save(`${out}.json`, result);
  return result;
}
