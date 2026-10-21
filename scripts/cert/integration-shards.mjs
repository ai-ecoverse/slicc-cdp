import { isDeepStrictEqual } from 'node:util';
import { root } from './common.mjs';
import { guardEvidenceProtocol, guardToolsProtocol } from './guard-protocol.mjs';
import { aggregateGuardReports } from './guard-shard-report.mjs';
import { guardShardPlan } from './guard-shards.mjs';

export function integrationToolsProtocol(commit, directory = root) {
  return guardToolsProtocol(
    commit,
    directory,
    '7efce0143c4f565672da5665fa8e9b9683a67807',
    'scripts/cert/integration-suite.mjs'
  );
}

export function validateIntegrationShards(
  execution,
  expected,
  vectors,
  run,
  label,
  emit,
  candidate = true,
  producingTools = integrationToolsProtocol(expected.harness.tools)
) {
  if (!execution?.files?.includes('test/integration/guard-runner.test.mjs')) return;
  const projected = { ...execution, report: execution.guardReport?.report };
  const protocol = guardEvidenceProtocol(projected, producingTools);
  if (protocol === 'LEGACY') return;
  if (protocol !== 'CURRENT' || !vectors) {
    emit('INCOMPLETE', `${label}: integration shard protocol or verified corpus absent`);
    return;
  }
  const plan = { ...guardShardPlan(vectors), head: expected.subject.commit, run };
  if (!isDeepStrictEqual(execution.guardShards.plan, plan))
    emit('INCOMPLETE', `${label}: shard plan contradicts independent staged corpus/source`);
  const aggregate = aggregateGuardReports(vectors, plan, execution.shards);
  for (const reason of aggregate.coverage.incomplete) emit('INCOMPLETE', `${label}: ${reason}`);
  for (const reason of aggregate.coverage.failures)
    if (candidate || !/^guard shard [a-d] failed$|^guard shard rows failed$/.test(reason))
      emit('FAIL', `${label}: ${reason}`);
  if (!isDeepStrictEqual(execution.guardReport?.report, aggregate.report))
    emit('FAIL', `${label}: integration aggregate differs from captured shard rows`);
}
