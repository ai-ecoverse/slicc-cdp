import { parseArgs } from 'node:util';
import { save } from './common.mjs';
import { suite } from './suite.mjs';
import { executionBudget } from './suite-policy.mjs';

const { values } = parseArgs({
  options: {
    directory: { type: 'string' },
    files: { type: 'string' },
    out: { type: 'string' },
    retry: { type: 'boolean', default: true },
    'worker-timeout-ms': { type: 'string' },
    'budget-ms': { type: 'string' },
  },
  allowNegative: true,
});
const budget = executionBudget({ allowMissing: true });
const result = await suite({
  deadline: budget.deadline ?? 0,
  workerTimeoutMs: values['worker-timeout-ms'] ? Number(values['worker-timeout-ms']) : undefined,
  budgetMs: values['budget-ms'] ? Number(values['budget-ms']) : undefined,
  cwd: values.directory,
  files: JSON.parse(values.files),
  out: values.out,
  retry: values.retry,
});
result.budget = budget;
save(`${values.out}.json`, result);
console.log(JSON.stringify(result));
process.exitCode = result.status === 'PASS' ? 0 : 1;
