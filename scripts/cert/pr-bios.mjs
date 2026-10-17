import { args, identity, json, save } from './common.mjs';

const options = args();
const plan = json(options.plan);
const pin = json(new URL('./pr-bios.json', import.meta.url));
save(options.out, {
  ...identity(plan),
  which: 'bios',
  status: 'INCOMPLETE',
  reason: 'bios canary workflow_call contract and SHA pin pending; no guarded-path smoke executed',
  pin,
  subjects: [
    { side: 'base', ...plan.baseline },
    { side: 'candidate', version: plan.version, commit: plan.commit },
  ],
});
