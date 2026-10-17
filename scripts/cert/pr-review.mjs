import { readFileSync } from 'node:fs';
import { args, identity, json, save } from './common.mjs';
import { hash } from './pr-common.mjs';

function toolUse(value) {
  if (!value || typeof value !== 'object') return false;
  return value.type === 'tool_use' || Object.values(value).some(toolUse);
}

const options = args({ verdict: { type: 'string' }, execution: { type: 'string' } });
const plan = json(options.plan);
let verdict;
let execution;
let reason;
try {
  const bytes = readFileSync(options.execution);
  const transcript = JSON.parse(bytes);
  const messages = Array.isArray(transcript) ? transcript : [transcript];
  if (toolUse(transcript)) throw new Error('review model had tools');
  const inits = messages.filter(
    (message) => message?.type === 'system' && message.subtype === 'init'
  );
  if (
    inits.some(
      (init) =>
        (Array.isArray(init.tools) && init.tools.length) ||
        (Array.isArray(init.mcp_servers) && init.mcp_servers.length)
    )
  )
    throw new Error('review model had tools');
  if (inits.length !== 1 || !Array.isArray(inits[0].tools) || !Array.isArray(inits[0].mcp_servers))
    throw new Error('review model tool receipt missing or malformed');
  const results = messages.filter((message) => message?.type === 'result');
  const result = results[0];
  if (
    process.env.CERT_REVIEW_RESULT !== 'success' ||
    results.length !== 1 ||
    messages.at(-1) !== result ||
    result.subtype !== 'success' ||
    result.is_error !== false ||
    typeof result.result !== 'string'
  )
    throw new Error('review execution did not succeed with one final result');
  const text = result.result.trim();
  const fenced = /^```json\s*\n([\s\S]*?)\n```$/.exec(text);
  verdict = JSON.parse(fenced ? fenced[1] : text);
  if (
    !verdict ||
    Array.isArray(verdict) ||
    typeof verdict !== 'object' ||
    Object.keys(verdict).some(
      (key) => !['status', 'head', 'commit', 'reason', 'known'].includes(key)
    ) ||
    !['PASS', 'FAIL', 'INCOMPLETE'].includes(verdict.status) ||
    verdict.head !== plan.head ||
    verdict.commit !== plan.commit ||
    typeof verdict.reason !== 'string' ||
    !verdict.reason.trim() ||
    (verdict.known !== undefined &&
      (!Array.isArray(verdict.known) || verdict.known.some((item) => typeof item !== 'string')))
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
