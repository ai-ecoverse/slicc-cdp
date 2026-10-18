import { existsSync } from 'node:fs';
import path from 'node:path';
import { cliVerdict } from './cli-verdict.mjs';
import { args, identity, json, root, save } from './common.mjs';
import { artifactFindings, artifactInventory, artifactNames } from './pr-artifact-binding.mjs';
import { biosCalls, biosFindings, selectBiosPin } from './pr-bios-result.mjs';
import { client } from './pr-common.mjs';
import { unpackLocal } from './pr-subject.mjs';

const printVerdict = cliVerdict('pr-bios: ');
const options = args({
  base: { type: 'string' },
  candidate: { type: 'string' },
  bindings: { type: 'string' },
});
const plan = json(options.plan);
const bound = options.bindings !== undefined;
const pin = selectBiosPin(json(new URL('./pr-bios.json', import.meta.url)), bound);
const result = {
  ...identity(plan),
  which: 'bios',
  pin,
  status: 'INCOMPLETE',
  results: {},
  ...(bound ? { outputs: {} } : {}),
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
  const context = {};
  if (bound) {
    context.bindings = json(options.bindings);
    const { api } = client(plan.repository);
    context.inventory = await artifactInventory(api, plan.run);
    result.artifactBinding = artifactFindings(plan, context.bindings, context.inventory, {
      roles: [
        'pack-base',
        'pack-candidate',
        ...Object.keys(artifactNames(plan)).filter((role) => role.startsWith('bios-')),
      ],
    });
  }
  result.calls = biosCalls(plan, pin, subjects, context.bindings);
  for (const side of ['base', 'candidate']) {
    const file = options[side] ?? path.join(path.dirname(options.plan), `${side}.json`);
    if (existsSync(file)) {
      const output = json(file);
      if (bound) {
        result.outputs[side] = output;
        result.results[side] =
          typeof output.result === 'string' ? JSON.parse(output.result) : output.result;
      } else result.results[side] = output;
    }
  }
  Object.assign(result, biosFindings(result, pin, plan, subjects, context));
  result.status = result.failures.length
    ? 'FAIL'
    : result.incomplete.length
      ? 'INCOMPLETE'
      : 'PASS';
  result.reason = [...result.failures, ...result.incomplete].join('; ') || undefined;
} catch (error) {
  result.status = result.artifactBinding?.failures.length
    ? 'FAIL'
    : (error.certStatus ?? (bound && error instanceof SyntaxError ? 'FAIL' : 'INCOMPLETE'));
  result.reason = error.message;
}
save(options.out, result);
if (result.status === 'FAIL') process.exitCode = 1;
printVerdict(result.status, options.out);
