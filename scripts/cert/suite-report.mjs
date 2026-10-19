import { createHash } from 'node:crypto';
import { closeSync, constants, fstatSync, openSync, readSync } from 'node:fs';

export function captureGuardReport(file) {
  let descriptor;
  try {
    descriptor = openSync(file, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
    const stat = fstatSync(descriptor);
    if (!stat.isFile() || stat.size > 128 * 1024 * 1024)
      throw new Error('guard report must be a regular JSON file no larger than 128 MiB');
    const buffer = Buffer.alloc(stat.size + 1);
    let length = 0;
    while (length < buffer.length) {
      const read = readSync(descriptor, buffer, length, buffer.length - length, null);
      if (!read) break;
      length += read;
    }
    if (length !== stat.size || fstatSync(descriptor).size !== stat.size)
      throw new Error('guard report changed while being captured');
    const bytes = buffer.subarray(0, length);
    const document = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes));
    if (!document || typeof document !== 'object' || !Array.isArray(document.results))
      throw new Error('guard report must contain a complete results array');
    const { diagnostics, ...report } = document;
    return {
      status: 'CAPTURED',
      file,
      bytes: bytes.length,
      sha256: createHash('sha256').update(bytes).digest('hex'),
      report,
      diagnostics: {
        file,
        count: Array.isArray(diagnostics) ? diagnostics.length : null,
        embedded: false,
      },
    };
  } catch (error) {
    return { status: 'INCOMPLETE', file, reason: error.message };
  } finally {
    if (descriptor !== undefined) closeSync(descriptor);
  }
}
