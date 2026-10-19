import { copyFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { evidence, json, root, run, save } from './common.mjs';
import { stage } from './stage.mjs';
import { suite } from './suite.mjs';
import { executionBudget } from './suite-policy.mjs';

await evidence('guard', async (plan, options) => {
  const budget = executionBudget();
  if (budget.status !== 'AVAILABLE')
    return { status: 'INCOMPLETE', reason: budget.reason, budget, executions: {} };
  const { deadline } = budget;
  const executions = {};
  for (const [leg, version, commit] of [
    ['positive', plan.version, plan.commit],
    ['negative', plan.baseline.version, plan.baseline.commit],
  ]) {
    try {
      const staged = await stage(plan, version, commit, `guard-${leg}`, root);
      const examples = path.join(staged.source, 'crates/cdp-client/examples');
      mkdirSync(examples, { recursive: true });
      copyFileSync(
        path.join(root, 'crates/cdp-client/examples/guard_bridge.rs'),
        path.join(examples, 'guard_bridge.rs')
      );
      const testList = path.join(root, '.cert/guard-proof-tests.json');
      save(testList, plan.proofs.tests);
      run(process.execPath, [
        path.join(root, 'scripts/cert/proof-harness.mjs'),
        '--directory',
        staged.source,
        '--tests',
        testList,
      ]);
      const result = await suite({
        deadline,
        cwd: staged.source,
        files: [
          ...new Set([
            'test/integration/guard-runner.test.mjs',
            'test/integration/guard-native.test.mjs',
            ...plan.proofs.tests
              .filter((test) => test.source === 'guard' && !test.missing)
              .map((test) => test.file),
          ]),
        ],
        out: path.join(root, '.cert', `guard-${leg}/suite`),
        env: {
          ...staged.env,
          SLICC_GUARD_SHA: commit,
          SLICC_FIXTURE_BROWSER_API: pathToFileURL(path.join(root, 'src/browser.js')).href,
        },
      });
      const rows = result.guardReport?.report ?? null;
      executions[leg] = { ...result, report: rows, version, commit, sha256: staged.sha256 };
    } catch (error) {
      executions[leg] = { status: 'INCOMPLETE', reason: error.stack, version, commit };
    }
  }
  const positive = executions.positive;
  const negative = executions.negative;
  const rowKey = (row) => `${row.id}/${row.target}/${row.variant}`;
  const failures =
    negative.report?.results
      .filter((row) => [row.control, row.guarded, row.live].includes('fail'))
      .map(rowKey)
      .sort() ?? [];
  const expected = plan.proofs.tests
    .filter((test) => test.source === 'guard-vector')
    .map((test) => `${test.vector}/${test.target}/${test.variant}`)
    .sort();
  const current = path.join(root, '.cert/guard-current.json');
  const previous = path.join(root, '.cert/guard-previous.json');
  const checked = path.join(root, '.cert/guard-proofs.json');
  save(current, positive);
  save(previous, negative);
  run(process.execPath, [
    path.join(root, 'scripts/cert/check-proofs.mjs'),
    '--plan',
    options.plan,
    '--source',
    'guard',
    '--positive',
    current,
    '--negative',
    previous,
    '--out',
    checked,
  ]);
  const proofs = json(checked);
  const incomplete =
    !positive.report ||
    !negative.report ||
    positive.status === 'INCOMPLETE' ||
    negative.status === 'INCOMPLETE';
  return {
    status:
      incomplete || proofs.status === 'INCOMPLETE'
        ? 'INCOMPLETE'
        : positive.status !== 'PASS' || !positive.report.pass || proofs.status === 'FAIL'
          ? 'FAIL'
          : 'PASS',
    reason: incomplete
      ? 'both guard legs must emit complete row evidence'
      : JSON.stringify(failures) !== JSON.stringify(expected)
        ? 'NEGATIVE row failures do not match declared guard fixes'
        : undefined,
    sha256: positive.sha256,
    executions,
    budget,
    expected,
    failures,
    proofs,
  };
});
