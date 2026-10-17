import path from 'node:path';
import { cliVerdict } from './cli-verdict.mjs';
import { args, identity, json, root, save } from './common.mjs';
import { biosCalls } from './pr-bios-result.mjs';
import { unpackLocal } from './pr-subject.mjs';

const printVerdict = cliVerdict('pr-bios-inputs: ');
const options = args();
const plan = json(options.plan);
const pin = json(new URL('./pr-bios.json', import.meta.url));
const result = { ...identity(plan), which: 'bios-inputs', status: 'INCOMPLETE', ready: false, pin };
try {
  if (plan.mode !== 'pr' || !plan.go || !/^\d+$/.test(plan.run))
    throw new Error('eligible hosted PR plan required');
  const subjects = Object.fromEntries(
    ['base', 'candidate'].map((side) => {
      const subject =
        side === 'base' ? plan.baseline : { version: plan.version, commit: plan.commit };
      return [
        side,
        unpackLocal(plan, subject.version, path.join(root, `.cert/bios-input-${side}`)),
      ];
    })
  );
  result.calls = biosCalls(plan, pin, subjects);
  if (!/^[a-f0-9]{40}$/.test(pin.commit ?? ''))
    throw new Error('bios canary merge SHA pending; inputs prepared but no guarded smoke executed');
  result.ready = true;
  result.status = 'PASS';
} catch (error) {
  result.status = error.certStatus ?? 'INCOMPLETE';
  result.reason = error.message;
}
save(options.out, result);
console.log(JSON.stringify(result));
if (result.status === 'FAIL') process.exitCode = 1;
printVerdict(result.status, options.out);
