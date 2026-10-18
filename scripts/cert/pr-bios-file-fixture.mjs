import { strict as assert } from 'node:assert';
import { createHash } from 'node:crypto';
import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { cliVerdict } from './cli-verdict.mjs';

export const fixtureBytes = 160 * 1024;
export function biosFileFixture(side, bytes = fixtureBytes) {
  const record = {
    side,
    transportFixture: true,
    text: '\nEOF\n$(touch injected)\n`touch injected`\n"\'\\\u0000\u2028\u2029😀',
    padding: '',
  };
  const padding = bytes - Buffer.byteLength(JSON.stringify(record));
  if (padding < 0) throw new Error('fixture size too small');
  record.padding = 'x'.repeat(padding);
  return JSON.stringify(record);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const print = cliVerdict('pr-bios-file-fixture: ');
  const { values } = parseArgs({
    options: { action: { type: 'string' }, directory: { type: 'string' }, out: { type: 'string' } },
  });
  const files = {};
  for (const side of ['base', 'candidate']) {
    const expected = biosFileFixture(side);
    assert.equal(Buffer.byteLength(expected), fixtureBytes);
    if (values.action === 'generate') {
      if (!process.env.GITHUB_OUTPUT) throw new Error('GITHUB_OUTPUT required');
      appendFileSync(process.env.GITHUB_OUTPUT, `${side}=${expected}\n`);
    } else if (values.action === 'check') {
      if (!values.directory) throw new Error('directory required');
      const actual = readFileSync(path.join(values.directory, `bios-${side}.json`));
      assert.equal(actual.length, fixtureBytes);
      assert.equal(actual.toString(), expected);
    } else throw new Error('generate or check required');
    files[side] = {
      bytes: fixtureBytes,
      sha256: createHash('sha256').update(expected).digest('hex'),
    };
  }
  const result = {
    schema: 1,
    status: 'PASS',
    which: 'bios-file-fixture',
    action: values.action,
    files,
  };
  if (values.out) writeFileSync(values.out, `${JSON.stringify(result, null, 2)}\n`);
  print('PASS', JSON.stringify(result));
}
