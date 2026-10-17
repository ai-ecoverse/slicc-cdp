import { readFileSync } from 'node:fs';
import { args, identity, json, save } from './common.mjs';
import { hash } from './pr-common.mjs';

const options = args({ verdict: { type: 'string' }, execution: { type: 'string' } });
const plan = json(options.plan);
let verdict;
let execution;
let reason;
try {
  verdict = json(options.verdict);
  const bytes = readFileSync(options.execution);
  const transcript = JSON.parse(bytes);
  if (!bytes.length || !transcript || process.env.CERT_REVIEW_RESULT !== 'success')
    throw new Error('review execution did not succeed');
  if (
    !['PASS', 'FAIL', 'INCOMPLETE'].includes(verdict.status) ||
    verdict.head !== plan.head ||
    verdict.commit !== plan.commit ||
    typeof verdict.reason !== 'string' ||
    !verdict.reason
  )
    throw new Error('review verdict missing or bound to another subject');
  execution = { sha256: hash(bytes), bytes: bytes.length };
} catch (error) {
  reason = error.message;
}
save(options.out, {
  ...identity(plan),
  which: 'review',
  status: reason ? 'INCOMPLETE' : verdict.status,
  reason,
  verdict,
  execution,
  head: plan.head,
  commit: plan.commit,
});
