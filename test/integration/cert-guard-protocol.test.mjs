import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { guardEvidenceProtocol, guardToolsProtocol } from '../../scripts/cert/guard-protocol.mjs';

const worker = 'test/integration/guard-runner.test.mjs';

function history(t) {
  const directory = mkdtempSync(path.join(tmpdir(), 'guard-protocol-history-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const git = (...args) => execFileSync('git', args, { cwd: directory, encoding: 'utf8' }).trim();
  git('init', '-q');
  git('config', 'user.name', 'Protocol fixture');
  git('config', 'user.email', 'protocol@example.invalid');
  writeFileSync(path.join(directory, 'old-fields.json'), '{"monolithic":true}');
  git('add', '-A', '--', ':/');
  git('-c', 'commit.gpgsign=false', 'commit', '-qm', 'old producing tools');
  const old = git('rev-parse', 'HEAD');
  mkdirSync(path.join(directory, 'scripts/cert'), { recursive: true });
  writeFileSync(path.join(directory, 'scripts/cert/guard-suite.mjs'), 'export const schema = 2;');
  git('add', '-A', '--', ':/');
  git('-c', 'commit.gpgsign=false', 'commit', '-qm', 'current producing tools');
  return { directory, old, current: git('rev-parse', 'HEAD') };
}

function monolithic() {
  const report = {
    results: [{ id: 'historical-row', target: 'ordinary', variant: 'prompt', transport: 'cdp' }],
  };
  const captured = { status: 'CAPTURED', file: 'owned-old-report.json', report };
  return {
    files: [worker],
    rows: [{ type: 'test:pass', worker, name: 'old worker completed' }],
    attempts: [{ workers: [{ file: worker, guardReport: captured }] }],
    policy: { isolatedGuardWorker: true },
    guardReport: captured,
    report,
  };
}

test('current shard evidence without its plan is INCOMPLETE and cannot become legacy', () => {
  assert.equal(
    guardEvidenceProtocol({ guardShards: { schema: 2 }, shards: [] }, 'CURRENT'),
    'INCOMPLETE'
  );
  assert.equal(guardEvidenceProtocol({ shards: [] }, 'CURRENT'), 'INCOMPLETE');
});

test('legacy-shaped evidence from current producing tools is INCOMPLETE', (t) => {
  const h = history(t);
  const protocol = guardToolsProtocol(h.current, h.directory, h.old);
  assert.equal(protocol, 'CURRENT');
  assert.equal(guardEvidenceProtocol(monolithic(), protocol), 'INCOMPLETE');
});

test('positively identified single-corpus-worker evidence from pre-shard producing tools is accepted', (t) => {
  const h = history(t);
  const protocol = guardToolsProtocol(h.old, h.directory, h.old);
  assert.equal(protocol, 'LEGACY');
  assert.equal(guardEvidenceProtocol(monolithic(), protocol), 'LEGACY');
  const duplicate = monolithic();
  duplicate.attempts[0].workers.push(duplicate.attempts[0].workers[0]);
  assert.equal(guardEvidenceProtocol(duplicate, protocol), 'INCOMPLETE');
  const contradictory = monolithic();
  contradictory.shards = [];
  assert.equal(guardEvidenceProtocol(contradictory, protocol), 'INCOMPLETE');
});
