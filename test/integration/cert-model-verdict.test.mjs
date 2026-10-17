import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const plan = { head: 'a'.repeat(40), commit: 'b'.repeat(40) };
const verdict = { status: 'PASS', ...plan, reason: 'complete evidence', known: [] };
const result = (text, extra = {}) => ({
  type: 'result',
  subtype: 'success',
  is_error: false,
  result: text,
  ...extra,
});
const text = JSON.stringify(verdict);
const cases = [
  ['whole JSON object', [result(text)], 'PASS'],
  ['one fenced json block', [result(`\`\`\`json\n${text}\n\`\`\``)], 'PASS'],
  [
    'preceding init and assistant are not verdicts',
    [{ type: 'system', subtype: 'init' }, { type: 'assistant' }, result(text)],
    'PASS',
  ],
  ['FAIL verdict preserved', [result(JSON.stringify({ ...verdict, status: 'FAIL' }))], 'FAIL'],
  [
    'INCOMPLETE verdict preserved',
    [result(JSON.stringify({ ...verdict, status: 'INCOMPLETE' }))],
    'INCOMPLETE',
  ],
  ['prose before JSON', [result(`PASS: ${text}`)], 'INCOMPLETE'],
  ['prose after JSON', [result(`${text}\ncertified`)], 'INCOMPLETE'],
  ['prose before fence', [result(`Verdict\n\`\`\`json\n${text}\n\`\`\``)], 'INCOMPLETE'],
  [
    'two fenced blocks',
    [result(`\`\`\`json\n${text}\n\`\`\`\n\`\`\`json\n${text}\n\`\`\``)],
    'INCOMPLETE',
  ],
  ['multiple objects', [result(`${text}\n${text}`)], 'INCOMPLETE'],
  ['keywords alone', [result('PASS certified')], 'INCOMPLETE'],
  ['array instead of object', [result(`[${text}]`)], 'INCOMPLETE'],
  ['another head', [result(JSON.stringify({ ...verdict, head: 'c'.repeat(40) }))], 'INCOMPLETE'],
  [
    'another commit',
    [result(JSON.stringify({ ...verdict, commit: 'c'.repeat(40) }))],
    'INCOMPLETE',
  ],
  ['empty reason', [result(JSON.stringify({ ...verdict, reason: ' ' }))], 'INCOMPLETE'],
  ['extra schema field', [result(JSON.stringify({ ...verdict, certified: true }))], 'INCOMPLETE'],
  ['non-string known issue', [result(JSON.stringify({ ...verdict, known: [true] }))], 'INCOMPLETE'],
  ['null verdict', [result('null')], 'INCOMPLETE'],
  ['error result', [result(text, { is_error: true })], 'INCOMPLETE'],
  ['missing result text', [result(null)], 'INCOMPLETE'],
  ['unsuccessful result', [result(text, { subtype: 'error_max_turns' })], 'INCOMPLETE'],
  ['multiple results', [result(text), result(text)], 'INCOMPLETE'],
  ['result not final', [result(text), { type: 'assistant' }], 'INCOMPLETE'],
  ['no result', [{ type: 'assistant', message: text }], 'INCOMPLETE'],
];

for (const [name, receipt, expected] of cases) {
  test(`PR review CLI validates execution receipt: ${name}`, (t) => {
    const directory = mkdtempSync(path.join(tmpdir(), 'slicc-model-verdict-'));
    t.after(() => rmSync(directory, { recursive: true, force: true }));
    const file = (name) => path.join(directory, name);
    writeFileSync(file('plan.json'), JSON.stringify(plan));
    writeFileSync(file('execution.json'), JSON.stringify(receipt));
    const invocation = spawnSync(
      process.execPath,
      [
        fileURLToPath(new URL('../../scripts/cert/pr-review.mjs', import.meta.url)),
        '--plan',
        file('plan.json'),
        '--execution',
        file('execution.json'),
        '--out',
        file('review.json'),
      ],
      { encoding: 'utf8', env: { ...process.env, CERT_REVIEW_RESULT: 'success' } }
    );
    assert.equal(invocation.status, 0, invocation.stderr);
    const document = JSON.parse(readFileSync(file('review.json')));
    assert.equal(document.status, expected);
    assert.equal(document.head, plan.head);
    assert.equal(document.commit, plan.commit);
    if (expected !== 'INCOMPLETE') assert.match(document.execution.sha256, /^[a-f0-9]{64}$/);
  });
}
