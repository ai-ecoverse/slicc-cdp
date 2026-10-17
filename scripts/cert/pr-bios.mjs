import { existsSync } from 'node:fs';
import path from 'node:path';
import { args, identity, json, root, save } from './common.mjs';
import { biosCalls, biosFindings } from './pr-bios-result.mjs';
import { unpackLocal } from './pr-subject.mjs';

const options = args({ base: { type: 'string' }, candidate: { type: 'string' } });
const plan = json(options.plan);
const pin = json(new URL('./pr-bios.json', import.meta.url));
const result = {
  ...identity(plan),
  which: 'bios',
  pin,
  status: 'INCOMPLETE',
  results: {},
  subjects: [
    { side: 'base', ...plan.baseline },
    { side: 'candidate', version: plan.version, commit: plan.commit },
  ],
};
try {
  if (plan.mode !== 'pr' || !plan.go) throw new Error('eligible PR plan required');
  const subjects = Object.fromEntries(
    ['base', 'candidate'].map((side) => [
      side,
      unpackLocal(
        plan,
        side === 'base' ? plan.baseline.version : plan.version,
        path.join(root, `.cert/bios-result-${side}`)
      ),
    ])
  );
  result.calls = biosCalls(plan, pin, subjects);
  for (const side of ['base', 'candidate']) {
    const file = options[side] ?? path.join(path.dirname(options.plan), `${side}.json`);
    if (existsSync(file)) result.results[side] = json(file);
  }
  Object.assign(result, biosFindings(result, pin, plan, subjects));
  result.status = result.failures.length
    ? 'FAIL'
    : result.incomplete.length
      ? 'INCOMPLETE'
      : 'PASS';
  result.reason = [...result.failures, ...result.incomplete].join('; ') || undefined;
} catch (error) {
  result.status = error.certStatus ?? 'INCOMPLETE';
  result.reason = error.message;
}
save(options.out, result);
if (result.status === 'FAIL') process.exitCode = 1;
