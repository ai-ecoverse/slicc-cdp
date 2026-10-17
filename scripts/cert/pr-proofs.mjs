import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { args, json, root, run, save } from './common.mjs';

const options = args();
const plan = json(options.plan);
if (plan.mode !== 'pr' || !plan.go) throw new Error('proof derivation needs an eligible PR plan');
const directory = path.dirname(options.out);
const fromSubject = (file) =>
  JSON.parse(
    execFileSync('git', ['show', `${plan.commit}:${file}`], { cwd: root, encoding: 'utf8' })
  );
const mapping = path.join(directory, 'mapping.json');
const registry = path.join(directory, 'registry.json');
const associations = path.join(directory, 'associations.json');
save(mapping, fromSubject('.cert/proofs.json'));
const descriptors = fromSubject('scripts/cert/proof-tests.json');
for (const [id, test] of Object.entries(descriptors)) {
  if (
    !/^[a-zA-Z0-9.:/-]+$/.test(id) ||
    !['integration', 'guard'].includes(test.source) ||
    !/^test\/integration\/[^/]+\.test\.mjs$/.test(test.file ?? '') ||
    typeof test.title !== 'string' ||
    !test.title
  )
    throw new Error('invalid public named proof descriptor');
}
save(registry, descriptors);
const commits = run('git', ['rev-list', '--no-merges', `${plan.base}..${plan.commit}`], {
  cwd: root,
})
  .split('\n')
  .filter(Boolean);
save(
  associations,
  Object.fromEntries(
    commits.map((commit) => [commit, [{ number: plan.pr, base: { ref: 'main' } }]])
  )
);
const result = JSON.parse(
  run(process.execPath, [
    path.join(root, 'scripts/cert/proofs.mjs'),
    '--base',
    plan.base,
    '--head',
    plan.commit,
    '--pulls',
    associations,
    '--mapping',
    mapping,
    '--registry',
    registry,
    '--out',
    path.join(directory, 'derived.json'),
  ])
);
plan.proofs = result;
plan.fixes = result.fixes;
save(options.out, plan);
