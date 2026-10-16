import { readdirSync } from 'node:fs';
import path from 'node:path';
import { evidence, json, root, run, save } from './common.mjs';
import { stage } from './stage.mjs';
import { suite } from './suite.mjs';

await evidence('integration', async (plan, options) => {
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
  const tests = plan.proofs.tests.filter((test) => test.source === 'integration' && !test.missing);
  const proofFiles = [...new Set(tests.map((test) => test.file))];
  const testList = path.join(root, '.cert/proof-tests.json');
  save(testList, tests);
  const install = (directory) =>
    run(process.execPath, [
      path.join(root, 'scripts/cert/proof-harness.mjs'),
      '--directory',
      directory,
      '--tests',
      testList,
    ]);
  install(positiveTree.source);
  const proofPositive = proofFiles.length
    ? await suite({
        cwd: positiveTree.source,
        files: proofFiles,
        env: positiveTree.env,
        out: path.join(root, '.cert/integration-positive/proofs'),
      })
    : { status: 'PASS', files: [], rows: [], attempts: [] };
  const negativeTree = await stage(
    plan,
    plan.baseline.version,
    plan.baseline.commit,
    'integration-negative',
    positiveTree.source
  );
  install(negativeTree.source);
  const negative = proofFiles.length
    ? await suite({
        cwd: negativeTree.source,
        files: proofFiles,
        env: negativeTree.env,
        out: path.join(root, '.cert/integration-negative/proofs'),
      })
    : { status: 'PASS', files: [], rows: [], attempts: [] };
  const current = path.join(root, '.cert/proof-positive.json');
  const previous = path.join(root, '.cert/proof-negative.json');
  const checked = path.join(root, '.cert/proof-results.json');
  save(current, proofPositive);
  save(previous, negative);
  run(process.execPath, [
    path.join(root, 'scripts/cert/check-proofs.mjs'),
    '--plan',
    options.plan,
    '--positive',
    current,
    '--negative',
    previous,
    '--out',
    checked,
  ]);
  const proofs = json(checked);
  return {
    status:
      positive.status === 'INCOMPLETE' || proofs.status === 'INCOMPLETE'
        ? 'INCOMPLETE'
        : positive.status !== 'PASS' || proofs.status === 'FAIL'
          ? 'FAIL'
          : 'PASS',
    sha256: positiveTree.sha256,
    executions: { positive, proofs: proofPositive, negative },
    proofs,
    native: 'published release assets',
    wasm: 'published npm tarball',
    rustHarness: 'committed external integration drivers against the subject public library',
  };
});
