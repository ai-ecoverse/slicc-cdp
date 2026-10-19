import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { cliVerdict } from './cli-verdict.mjs';
import { args, identity, json, save } from './common.mjs';
import { artifactFindings, artifactInventory } from './pr-artifact-binding.mjs';
import { biosCalls, selectBiosPin } from './pr-bios-result.mjs';
import { client } from './pr-common.mjs';
import { unpackLocal } from './pr-subject.mjs';

const printVerdict = cliVerdict('pr-bios-inputs: ');
const options = args({ bindings: { type: 'string' }, directory: { type: 'string' } });
const plan = json(options.plan);
const bound = options.bindings !== undefined;
const pin = selectBiosPin(json(new URL('./pr-bios.json', import.meta.url)), bound);
const result = { ...identity(plan), which: 'bios-inputs', status: 'INCOMPLETE', ready: false, pin };
let temporary;
try {
  if (plan.mode !== 'pr' || !plan.go || !/^\d+$/.test(plan.run))
    throw new Error('eligible hosted PR plan required');
  let bindings;
  if (bound) {
    bindings = json(options.bindings);
    const { api } = client(plan.repository);
    const inventory = await artifactInventory(api, plan.run);
    result.artifactBinding = artifactFindings(plan, bindings, inventory, {
      roles: ['pack-base', 'pack-candidate'],
    });
    if (result.artifactBinding.status !== 'PASS')
      throw Object.assign(
        new Error(
          [...result.artifactBinding.failures, ...result.artifactBinding.incomplete].join('; ')
        ),
        { certStatus: result.artifactBinding.status }
      );
  }
  const directory =
    options.directory ?? (temporary = mkdtempSync(path.join(tmpdir(), 'slicc-bios-inputs-')));
  const subjects = Object.fromEntries(
    ['base', 'candidate'].map((side) => {
      const subject =
        side === 'base' ? plan.baseline : { version: plan.version, commit: plan.commit };
      return [side, unpackLocal(plan, subject.version, path.join(directory, side))];
    })
  );
  result.calls = biosCalls(plan, pin, subjects, bindings);
  if (!/^[a-f0-9]{40}$/.test(pin.commit ?? ''))
    throw new Error('bios canary merge SHA pending; inputs prepared but no guarded smoke executed');
  result.ready = true;
  result.status = 'PASS';
} catch (error) {
  result.status =
    error.certStatus ?? (bound && error instanceof SyntaxError ? 'FAIL' : 'INCOMPLETE');
  result.reason = error.message;
} finally {
  if (temporary) rmSync(temporary, { recursive: true, force: true });
}
save(options.out, result);
console.log(JSON.stringify(result));
if (result.status === 'FAIL') process.exitCode = 1;
printVerdict(result.status, options.out);
