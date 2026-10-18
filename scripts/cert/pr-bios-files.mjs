import { createHash } from 'node:crypto';
import { existsSync, lstatSync, mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { cliVerdict } from './cli-verdict.mjs';

export const sideLimit = 512 * 1024;
export const inputLimit = 8 * 1024 * 1024;
const sides = ['base', 'candidate'];
const document = (status, reason, files = {}) => ({
  schema: 1,
  which: 'bios-file-transport',
  status,
  reason,
  files,
  sideLimit,
  inputLimit,
});

export async function biosEnvelope(stream) {
  let bytes = 0;
  const chunks = [];
  for await (const chunk of stream) {
    const buffer = Buffer.from(chunk);
    bytes += buffer.length;
    if (bytes <= inputLimit) chunks.push(buffer);
  }
  if (bytes > inputLimit) return { status: 'INCOMPLETE', reason: 'stdin exceeds byte cap' };
  if (!bytes) return { status: 'INCOMPLETE', reason: 'stdin absent' };
  try {
    return {
      value: JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(chunks))),
    };
  } catch {
    return { status: 'FAIL', reason: 'stdin is not a UTF-8 JSON envelope' };
  }
}

export function writeBiosFiles(envelope, directory) {
  if (!envelope || typeof envelope !== 'object' || Array.isArray(envelope))
    return document('FAIL', 'object envelope required');
  if (Object.keys(envelope).some((key) => !sides.includes(key)))
    return document('FAIL', 'unexpected envelope field');
  const buffers = {};
  for (const side of sides) {
    const value = envelope[side];
    if (value === undefined || value === null || value === '' || value === 'null')
      return document('INCOMPLETE', `${side}: result absent`);
    if (typeof value !== 'string') return document('FAIL', `${side}: JSON string required`);
    const buffer = Buffer.from(value);
    if (buffer.length > sideLimit)
      return document('INCOMPLETE', `${side}: result exceeds byte cap`);
    try {
      const result = JSON.parse(value);
      if (!result || typeof result !== 'object' || Array.isArray(result))
        return document('FAIL', `${side}: result object required`);
    } catch {
      return document('FAIL', `${side}: result JSON malformed`);
    }
    buffers[side] = buffer;
  }
  const files = {};
  try {
    mkdirSync(directory, { recursive: true });
    const entry = lstatSync(directory);
    if (!entry.isDirectory() || entry.isSymbolicLink())
      return document('FAIL', 'unsafe output directory');
    for (const side of sides) {
      const file = `bios-${side}.json`;
      const target = path.join(directory, file);
      if (
        existsSync(target) ||
        (() => {
          try {
            lstatSync(target);
            return true;
          } catch (error) {
            if (error.code !== 'ENOENT') throw error;
            return false;
          }
        })()
      )
        return document('FAIL', `${side}: output already exists`);
      files[side] = {
        file,
        bytes: buffers[side].length,
        sha256: createHash('sha256').update(buffers[side]).digest('hex'),
      };
    }
    for (const side of sides)
      writeFileSync(path.join(directory, files[side].file), buffers[side], {
        flag: 'wx',
        mode: 0o600,
      });
  } catch (error) {
    return document('FAIL', `file write refused: ${error.code ?? error.message}`);
  }
  return document('PASS', 'complete JSON bytes written; transport only, not a bios verdict', files);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const print = cliVerdict('pr-bios-files: ');
  const { values } = parseArgs({
    options: { directory: { type: 'string' }, out: { type: 'string' } },
  });
  if (!values.directory) throw new Error('directory required');
  const input = await biosEnvelope(process.stdin);
  const result = input.status
    ? document(input.status, input.reason)
    : writeBiosFiles(input.value, values.directory);
  if (values.out) writeFileSync(values.out, `${JSON.stringify(result, null, 2)}\n`);
  print(result.status, JSON.stringify(result));
  if (result.status !== 'PASS') process.exitCode = 1;
}
