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
const init = { type: 'system', subtype: 'init', tools: [], mcp_servers: [] };
const text = JSON.stringify(verdict);
const cases = [
  ['whole JSON object', [result(text)], 'PASS'],
  ['one fenced json block', [result(`\`\`\`json\n${text}\n\`\`\``)], 'PASS'],
  [
    'preceding init and assistant are not verdicts',
    [init, { type: 'assistant' }, result(text)],
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
  [
    'tool_use despite empty init',
    [
      init,
      {
        type: 'assistant',
        message: {
          content: [
            { type: 'tool_use', name: 'Read', id: 'fixture', input: { file_path: 'never-read' } },
          ],
        },
      },
      result(text),
    ],
    'INCOMPLETE',
  ],
  [
    'streamed tool_use despite empty init',
    [
      init,
      {
        type: 'stream_event',
        event: { content_block: { type: 'tool_use', name: 'Bash', id: 'fixture' } },
      },
      result(text),
    ],
    'INCOMPLETE',
  ],
  [
    'tool_use words in text are only data',
    [
      init,
      { type: 'assistant', message: { content: [{ type: 'text', text: 'tool_use Read' }] } },
      result(text),
    ],
    'PASS',
  ],
  ['missing init', [result(text)], 'INCOMPLETE'],
  ['Read tool in init', [{ ...init, tools: ['Read'] }, result(text)], 'INCOMPLETE'],
  [
    'MCP server in init',
    [{ ...init, mcp_servers: [{ name: 'fixture', status: 'connected' }] }, result(text)],
    'INCOMPLETE',
  ],
  [
    'missing tools list',
    [{ type: 'system', subtype: 'init', mcp_servers: [] }, result(text)],
    'INCOMPLETE',
  ],
  [
    'missing MCP list',
    [{ type: 'system', subtype: 'init', tools: [] }, result(text)],
    'INCOMPLETE',
  ],
  ['duplicate empty init', [init, init, result(text)], 'INCOMPLETE'],
  [
    'Read init cannot be hidden by a second empty init',
    [{ ...init, tools: ['Read'] }, init, result(text)],
    'INCOMPLETE',
  ],
  ['no result', [{ type: 'assistant', message: text }], 'INCOMPLETE'],
];

for (const [name, receipt, expected] of cases) {
  test(`PR review CLI validates execution receipt: ${name}`, (t) => {
    const directory = mkdtempSync(path.join(tmpdir(), 'slicc-model-verdict-'));
    t.after(() => rmSync(directory, { recursive: true, force: true }));
    const file = (name) => path.join(directory, name);
    writeFileSync(file('plan.json'), JSON.stringify(plan));
    const messages =
      name === 'missing init' ||
      receipt.some((message) => message.type === 'system' && message.subtype === 'init')
        ? receipt
        : [init, ...receipt];
    writeFileSync(file('execution.json'), JSON.stringify(messages));
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
    if (name.includes('Read') || name === 'MCP server in init' || name.includes('tool_use despite'))
      assert.equal(document.reason, 'review model had tools');
    if (name === 'missing init') assert.match(document.reason, /tool receipt missing/);
    if (expected !== 'INCOMPLETE') assert.match(document.execution.sha256, /^[a-f0-9]{64}$/);
  });
}
