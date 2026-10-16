import { appendFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { capture, identity, json, save } from './common.mjs';
import { registry } from './registry.mjs';

const { values } = parseArgs({
  options: { evidence: { type: 'string' }, out: { type: 'string' } },
});
const planFile = path.join(values.evidence, 'plan.json');
const plan = json(planFile);
plan.upstream = JSON.parse(process.env.CERT_UPSTREAM ?? '{}');
save(planFile, plan);
let verification;
try {
  verification = await registry(plan);
} catch (error) {
  verification = { status: 'INCOMPLETE', reason: error.stack };
}
save(path.join(values.evidence, 'registry.json'), {
  ...identity(plan),
  which: 'registry',
  ...verification,
});
const recordFile = path.join(values.out, 'record.json');
const result = capture(process.execPath, [
  path.join(import.meta.dirname, 'conclude.mjs'),
  '--evidence',
  values.evidence,
  '--out',
  recordFile,
]);
let record;
try {
  record = json(recordFile);
} catch {
  record = {
    ...identity(plan),
    status: 'INCOMPLETE',
    certified: false,
    line: `cert-ci: INCOMPLETE @ai-ecoverse/slicc-cdp@${plan.version} (record validator: ${result.stderr})`,
  };
  save(recordFile, record);
}
writeFileSync(path.join(values.out, 'line.txt'), `${record.line}\n`);
if (process.env.GITHUB_STEP_SUMMARY)
  appendFileSync(
    process.env.GITHUB_STEP_SUMMARY,
    `${record.line}\n\nEvidence: ${record.runUrl ?? ''}\n`
  );
if (process.env.GITHUB_OUTPUT)
  appendFileSync(
    process.env.GITHUB_OUTPUT,
    `certified=${record.certified}\nsha256=${record.sha256 ?? ''}\nstate=${record.certified ? 'success' : record.status === 'INCOMPLETE' ? 'error' : 'failure'}\n`
  );
console.log(record.line);
