import { spawnSync } from 'node:child_process';
import { isDeepStrictEqual } from 'node:util';
import { root } from './common.mjs';

const beforeShards = '615e5f2663412f444b32f9ef96e6654c8dddef92';
const guardFile = 'test/integration/guard-runner.test.mjs';

export function guardToolsProtocol(commit, directory = root, cutoff = beforeShards) {
  if (!/^[a-f0-9]{40}$/.test(commit ?? '') || !/^[a-f0-9]{40}$/.test(cutoff)) return 'UNKNOWN';
  const git = (args) =>
    spawnSync('git', args, { cwd: directory, timeout: 120000, encoding: 'utf8' });
  if (git(['cat-file', '-e', `${commit}:scripts/cert/guard-suite.mjs`]).status === 0)
    return 'CURRENT';
  const ancestry = git(['merge-base', '--is-ancestor', commit, cutoff]);
  if (ancestry.status === 0) return 'LEGACY';
  return ancestry.status === 1 ? 'CURRENT' : 'UNKNOWN';
}

function legacyShape(execution) {
  if (
    execution?.shards !== undefined ||
    execution?.shardPlan !== undefined ||
    execution?.native !== undefined ||
    execution?.shardCoverage !== undefined
  )
    return false;
  if (
    !Array.isArray(execution?.files) ||
    !execution.files.includes(guardFile) ||
    !Array.isArray(execution.rows) ||
    !execution.rows.length ||
    !Array.isArray(execution.attempts) ||
    !execution.attempts.length ||
    execution.policy?.isolatedGuardWorker !== true ||
    execution.guardReport?.status !== 'CAPTURED' ||
    typeof execution.guardReport.file !== 'string' ||
    !isDeepStrictEqual(execution.report, execution.guardReport.report)
  )
    return false;
  const workers = [];
  for (const attempt of execution.attempts) {
    if (!Array.isArray(attempt?.workers)) return false;
    const guarded = attempt.workers.filter((worker) => worker?.file === guardFile);
    if (guarded.length > 1) return false;
    workers.push(...guarded);
  }
  const last = workers.at(-1);
  return (
    last?.guardReport?.status === 'CAPTURED' &&
    last.guardReport.file === execution.guardReport.file &&
    isDeepStrictEqual(last.guardReport.report, execution.report)
  );
}

export function guardEvidenceProtocol(execution, producingTools) {
  if (producingTools === 'UNKNOWN') return 'INCOMPLETE';
  const marker = execution?.guardShards;
  if (producingTools === 'LEGACY')
    return marker === undefined && legacyShape(execution) ? 'LEGACY' : 'INCOMPLETE';
  if (
    producingTools !== 'CURRENT' ||
    marker?.schema !== 2 ||
    !marker.plan ||
    typeof marker.plan !== 'object' ||
    Array.isArray(marker.plan) ||
    execution.shardPlan !== undefined ||
    !Array.isArray(execution.shards)
  )
    return 'INCOMPLETE';
  return 'CURRENT';
}
