import { readdirSync } from 'node:fs';
import path from 'node:path';
import { evidence, root } from './common.mjs';
import { stage } from './stage.mjs';
import { suite } from './suite.mjs';

await evidence('integration', async (plan) => {
  const positiveTree = await stage(plan, plan.version, plan.commit, 'integration-positive');
  const files = readdirSync(path.join(positiveTree.source, 'test/integration'))
    .filter(
      (file) =>
        file.endsWith('.test.mjs') && !file.startsWith('cert-') && file !== 'toolchain.test.mjs'
    )
    .map((file) => `test/integration/${file}`);
  const positive = await suite({
    cwd: positiveTree.source,
    files,
    env: positiveTree.env,
    out: path.join(root, '.cert/integration-positive/suite'),
  });
  const negativeTree = await stage(
    plan,
    plan.baseline.version,
    plan.baseline.commit,
    'integration-negative',
    positiveTree.source
  );
  const integrationFixes = plan.fixes.filter(
    (fix) => !fix.tests.length || fix.tests.some((test) => test.source !== 'guard')
  );
  const negativeFiles = [
    ...new Set(
      integrationFixes.flatMap((fix) =>
        fix.tests.filter((test) => test.source !== 'guard').map((test) => test.file)
      )
    ),
  ];
  const negative = negativeFiles.length
    ? await suite({
        cwd: negativeTree.source,
        files: negativeFiles,
        env: negativeTree.env,
        out: path.join(root, '.cert/integration-negative/suite'),
      })
    : {
        status: 'PASS',
        files: [],
        rows: [],
        attempts: [],
        reason: 'no parsed fix integration files',
      };
  const fixes = integrationFixes.map((fix) => {
    if (!fix.tests.length)
      return {
        ...fix,
        status: 'INCOMPLETE',
        reason: 'fix has no parsed behaviour-level integration test',
      };
    const results = fix.tests.map((test) => {
      const find = (execution) =>
        execution.rows.filter((row) => row.name === test.title && row.file?.endsWith(test.file));
      const current = find(positive);
      const previous = find(negative);
      const passed = current.some((row) => row.type === 'test:pass' && !row.skip && !row.todo);
      const failed = previous.some(
        (row) =>
          row.type === 'test:fail' &&
          row.failureType === 'testCodeFailure' &&
          !/running 0 tests|could not compile|ERR_MODULE_NOT_FOUND|Cannot find module/.test(
            row.error ?? ''
          )
      );
      return {
        ...test,
        status:
          !current.length || !previous.length
            ? 'INCOMPLETE'
            : !passed
              ? 'FAIL'
              : failed
                ? 'PASS'
                : 'NO-NEGATIVE',
        current,
        previous,
      };
    });
    return {
      ...fix,
      status: results.some((result) => result.status === 'INCOMPLETE')
        ? 'INCOMPLETE'
        : results.some((result) => result.status === 'FAIL')
          ? 'FAIL'
          : results.some((result) => result.status === 'PASS')
            ? 'PASS'
            : 'INCOMPLETE',
      results,
    };
  });
  return {
    status:
      positive.status === 'INCOMPLETE' ||
      negative.status === 'INCOMPLETE' ||
      fixes.some((fix) => fix.status === 'INCOMPLETE')
        ? 'INCOMPLETE'
        : positive.status === 'PASS' && fixes.every((fix) => fix.status === 'PASS')
          ? 'PASS'
          : 'FAIL',
    sha256: positiveTree.sha256,
    executions: { positive, negative },
    fixes,
    native: 'published release assets',
    wasm: 'published npm tarball',
    rustHarness: 'subject source; committed integration wrappers may build their Rust test drivers',
  };
});
