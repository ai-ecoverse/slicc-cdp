import path from 'node:path';
import { parseArgs } from 'node:util';
import { capture, gh, json, repo, root, run, save } from './common.mjs';

const { values } = parseArgs({
  options: {
    directory: { type: 'string', default: root },
    base: { type: 'string' },
    head: { type: 'string' },
    out: { type: 'string' },
    pulls: { type: 'string' },
    mapping: { type: 'string' },
    registry: { type: 'string' },
  },
});
const cwd = values.directory;
const mapping = json(values.mapping ?? path.join(root, '.cert/proofs.json'));
const registry = json(values.registry ?? path.join(root, 'scripts/cert/proof-tests.json'));
const associations = values.pulls ? json(values.pulls) : null;
const commits = run(
  'git',
  ['rev-list', '--no-merges', '--reverse', `${values.base}..${values.head}`],
  { cwd }
)
  .split('\n')
  .filter(Boolean);
const merges = run(
  'git',
  [
    'log',
    '--first-parent',
    '--merges',
    '--reverse',
    '--format=%P %s',
    `${values.base}..${values.head}`,
  ],
  { cwd }
)
  .split('\n')
  .filter(Boolean)
  .map((line) => {
    const match = line.match(/^([a-f0-9]+) ([a-f0-9]+) .*?Merge pull request #(\d+)/);
    return match ? { before: match[1], after: match[2], number: Number(match[3]) } : null;
  })
  .filter(Boolean);
const fixes = [];
const known = [];
const named = new Set();
for (const commit of commits) {
  const subject = run('git', ['show', '-s', '--format=%s', commit], { cwd });
  if (!/^fix(?:\([^)]*\))?!?:/.test(subject) || /^fix\(sliccy-browser\):/.test(subject)) continue;
  const body = run('git', ['show', '-s', '--format=%b', commit], { cwd });
  const introduced = merges.find(
    (merge) =>
      capture('git', ['merge-base', '--is-ancestor', commit, merge.after], { cwd }).exit === 0 &&
      capture('git', ['merge-base', '--is-ancestor', commit, merge.before], { cwd }).exit !== 0
  );
  const pulls = introduced
    ? [introduced]
    : associations
      ? (associations[commit] ?? [])
      : gh(`repos/${repo}/commits/${commit}/pulls?per_page=100`);
  const pr =
    pulls.find((entry) => entry.merged_at && entry.base?.ref === 'main')?.number ??
    pulls[0]?.number ??
    null;
  const declarations = [...body.matchAll(/^Proves:\s*(.+)$/gm)].flatMap((match) =>
    match[1]
      .split(',')
      .map((id) => id.trim())
      .filter(Boolean)
  );
  const ids = [...new Set(declarations.length ? declarations : (mapping[String(pr)] ?? []))].sort();
  const fix = { commit, subject, pr, proofIds: ids };
  fixes.push(fix);
  if (!ids.length) known.push(`known test gap: unproven fix ${pr ? `#${pr}` : commit}`);
  for (const id of ids) named.add(id);
}
const tests = [...named].sort().map((id) => {
  const vector = id.match(/^vector:([a-zA-Z0-9.-]+)\/([a-zA-Z0-9-]+)\/([a-zA-Z0-9-]+)$/);
  const descriptor =
    registry[id] ??
    (vector
      ? { source: 'guard-vector', vector: vector[1], target: vector[2], variant: vector[3] }
      : null);
  return { id, ...descriptor, missing: !descriptor };
});
const expected = tests.map((test) => test.id);
const result = { fixes, tests, expected, known: [...new Set(known)] };
save(values.out, result);
console.log(JSON.stringify(result));
