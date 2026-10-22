import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';

const root = path.resolve(import.meta.dirname, '../..');
const command = JSON.parse(readFileSync(path.join(root, 'package.json'), 'utf8')).scripts[
  'test:integration'
];
const boundary = command.split('npm run -s browser >/dev/null && ')[1];

test('required integration check binds every matrix owner and the full-universe union', () => {
  const workflow = readFileSync(path.join(root, '.github/workflows/ci.yml'), 'utf8');
  const gate = workflow.slice(
    workflow.indexOf('  integration:\n'),
    workflow.indexOf('  integration-suite:\n')
  );
  assert.ok(
    gate.includes('needs: [integration-suite, guard-partitions, guard-union, redirect-regressions]')
  );
  assert.ok(gate.includes('if: always()'));
  assert.ok(gate.includes('SUITE_RESULT: ${{ needs.integration-suite.result }}'));
  assert.ok(gate.includes('REDIRECT_RESULT: ${{ needs.redirect-regressions.result }}'));
  assert.ok(gate.includes('GUARD_RESULT: ${{ needs.guard-partitions.result }}'));
  assert.ok(gate.includes('UNION_RESULT: ${{ needs.guard-union.result }}'));
  const matrix = workflow.slice(
    workflow.indexOf('  guard-partitions:\n'),
    workflow.indexOf('  guard-union:\n')
  );
  assert.ok(matrix.includes('name: guard-${{ matrix.shard }}'));
  assert.ok(matrix.includes('shard: [a, b, c, d]'));
  assert.ok(matrix.includes('fail-fast: false'));
  const commands = gate.split('\n').filter((line) => line.trim().startsWith('run: '));
  assert.equal(commands.length, 1);
  const command = commands[0].trim().slice('run: '.length);
  for (const owner of ['SUITE_RESULT', 'REDIRECT_RESULT', 'GUARD_RESULT', 'UNION_RESULT']) {
    for (const status of ['success', 'failure', 'cancelled', 'skipped', '', 'neutral']) {
      const env = {
        ...process.env,
        SUITE_RESULT: 'success',
        REDIRECT_RESULT: 'success',
        GUARD_RESULT: 'success',
        UNION_RESULT: 'success',
        [owner]: status,
      };
      const result = spawnSync('sh', ['-c', command], { env });
      assert.equal(result.status === 0, status === 'success', `${owner}/${status}`);
    }
  }
});

for (const mode of ['invalid-job', 'missing-selector']) {
  test(`integration selector ${mode} fails before the test runner starts`, () => {
    const directory = mkdtempSync(path.join(tmpdir(), 'integration-boundary-'));
    const marker = path.join(directory, 'runner-started');
    const node = path.join(directory, 'node');
    writeFileSync(
      node,
      '#!/bin/sh\nif [ "$1" = scripts/test-integration.mjs ]; then\n  printf ran > "$BOUNDARY_MARKER"\n  exit 0\nfi\nexec "$BOUNDARY_NODE" "$@"\n'
    );
    chmodSync(node, 0o755);
    try {
      assert.ok(boundary, 'integration command must expose its selector boundary');
      const result = spawnSync('sh', ['-c', boundary], {
        cwd: mode === 'missing-selector' ? directory : root,
        encoding: 'utf8',
        env: {
          ...process.env,
          PATH: `${directory}${path.delimiter}${process.env.PATH}`,
          BOUNDARY_NODE: process.execPath,
          BOUNDARY_MARKER: marker,
          SLICC_INTEGRATION_JOB: 'invalid-job',
        },
      });
      assert.notEqual(result.status, 0, result.stderr);
      assert.equal(existsSync(marker), false, 'selector failure reached Node test autodiscovery');
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
}
