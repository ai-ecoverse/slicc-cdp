import assert from 'node:assert/strict';

export function assertVerdict(stdout, name, status) {
  const lines = stdout.split(/\r?\n/).filter((line) => line.startsWith(`${name}: `));
  assert.equal(lines.length, 1, stdout);
  assert.equal(lines[0].split(' ')[1], status, stdout);
  return lines[0];
}

export function stdoutDocument(stdout) {
  return JSON.parse(stdout.split(/\r?\n/).find((line) => line.startsWith('{')));
}
