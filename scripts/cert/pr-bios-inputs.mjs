import path from 'node:path';
import { args, identity, json, root, save } from './common.mjs';
import { unpackLocal } from './pr-subject.mjs';

const options = args();
const plan = json(options.plan);
const pin = json(new URL('./pr-bios.json', import.meta.url));
const result = { ...identity(plan), which: 'bios-inputs', status: 'INCOMPLETE', ready: false, pin };
try {
  if (plan.mode !== 'pr' || !plan.go || !/^\d+$/.test(plan.run))
    throw new Error('eligible hosted PR plan required');
  result.calls = Object.fromEntries(
    ['base', 'candidate'].map((side) => {
      const subject =
        side === 'base' ? plan.baseline : { version: plan.version, commit: plan.commit };
      const pack = unpackLocal(plan, subject.version, path.join(root, `.cert/bios-input-${side}`));
      return [
        side,
        {
          label: side,
          overrides: '[]',
          runner: 'ubuntu-latest',
          shards: JSON.stringify(pin.shards),
          'bios-ref': pin.commit,
          'candidate-package': '@ai-ecoverse/slicc-cdp',
          'candidate-artifact': `cert-pr-pack-${plan.pr}-${plan.head}-${plan.attempt}-${side}`,
          'candidate-run': plan.run,
          'candidate-sha256': pack.sha256,
          'candidate-repo': plan.repository,
          repeat: 1,
        },
      ];
    })
  );
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
