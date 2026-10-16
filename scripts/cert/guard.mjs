import { copyFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import { evidence, json, root } from './common.mjs';
import { stage } from './stage.mjs';
import { suite } from './suite.mjs';

await evidence('guard', async (plan) => {
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
      const report = path.join(root, '.cert', `guard-${leg}/rows.md`);
      const result = await suite({
        cwd: staged.source,
        files: ['test/integration/guard-runner.test.mjs', 'test/integration/guard-native.test.mjs'],
        out: path.join(root, '.cert', `guard-${leg}/suite`),
        env: { ...staged.env, SLICC_GUARD_REPORT: report, SLICC_GUARD_SHA: commit },
      });
      let rows;
      try {
        rows = json(`${report}.json`);
      } catch {
        rows = null;
      }
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
  const expected = plan.fixes.flatMap((fix) => fix.guardRows ?? []).sort();
  const incomplete =
    !positive.report ||
    !negative.report ||
    positive.status === 'INCOMPLETE' ||
    negative.status === 'INCOMPLETE';
  return {
    status:
      incomplete || JSON.stringify(failures) !== JSON.stringify(expected)
        ? 'INCOMPLETE'
        : positive.status !== 'PASS' || !positive.report.pass
          ? 'FAIL'
          : 'PASS',
    reason: incomplete
      ? 'both guard legs must emit complete row evidence'
      : JSON.stringify(failures) !== JSON.stringify(expected)
        ? 'NEGATIVE row failures do not match declared guard fixes'
        : undefined,
    sha256: positive.sha256,
    executions,
    expected,
    failures,
  };
});
