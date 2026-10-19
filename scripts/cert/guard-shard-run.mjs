import path from 'node:path';
import { parseArgs } from 'node:util';
import { json, root, save } from './common.mjs';
import { guardSetupTimeoutCount } from './guard-shards.mjs';
import { suite } from './suite.mjs';
import { executionBudget } from './suite-policy.mjs';

const { values } = parseArgs({
  options: { plan: { type: 'string' }, shard: { type: 'string' }, out: { type: 'string' } },
});
const plan = json(values.plan);
const shard = plan.shards.find((shard) => shard.id === values.shard);
if (!shard) throw new Error('guard shard must be declared by the plan');
const budget = executionBudget();
const result =
  budget.status === 'AVAILABLE'
    ? await suite({
        cwd: root,
        files: ['test/integration/guard-runner.test.mjs'],
        out: path.join(path.dirname(values.out), 'suite'),
        deadline: budget.deadline,
        env: { SLICC_GUARD_TARGETS: JSON.stringify(shard.targets), SLICC_GUARD_PROGRESS: '1' },
        retry: false,
      })
    : { status: 'INCOMPLETE', reason: budget.reason, attempts: [] };
const report = result.guardReport?.report;
const setupTimeoutCount = guardSetupTimeoutCount(report);
const document = {
  schema: 1,
  id: shard.id,
  targets: shard.targets,
  head: plan.head,
  run: plan.run,
  budget,
  suite: result,
  setupTimeoutCount,
  wallTimeMs: budget.jobStartedMs ? Date.now() - budget.jobStartedMs : result.elapsedMs,
  measurementOnly: true,
};
save(values.out, document);
console.log(
  JSON.stringify({
    id: shard.id,
    status: result.status,
    wallTimeMs: document.wallTimeMs,
    suiteTimeMs: result.elapsedMs,
    setupTimeoutCount,
    rows: report?.results?.length ?? 0,
  })
);
process.exitCode = result.status === 'PASS' ? 0 : 1;
