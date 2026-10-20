import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import yaml from 'js-yaml';
import { artifactNames } from '../../scripts/cert/pr-artifact-binding.mjs';
import { biosAdvisoryLedger, biosLedgerObservations } from '../../scripts/cert/pr-bios-ledger.mjs';
import { pairedBiosFindings } from '../../scripts/cert/pr-bios-pair.mjs';
import { biosCalls, selectBiosPin } from '../../scripts/cert/pr-bios-result.mjs';
import {
  biosShardContract,
  biosShards,
  fetchBiosShardContract,
} from '../../scripts/cert/pr-bios-shards.mjs';

const pin = JSON.parse(readFileSync(new URL('../../scripts/cert/pr-bios.json', import.meta.url)));
const snapshot = JSON.parse(
  readFileSync(new URL('./cert-bios-shard-catalog.json', import.meta.url))
);
const bytes = Buffer.from(snapshot.content);
const catalog = JSON.parse(snapshot.content);
const workflow = yaml.load(
  readFileSync(new URL('../../.github/workflows/cert-review-pr.yml', import.meta.url), 'utf8')
);
const plan = {
  repository: 'fixture/lane',
  run: '42',
  attempt: '1',
  pr: 1,
  head: 'a'.repeat(40),
  commit: 'b'.repeat(40),
};

function writer(value, t) {
  const directory = mkdtempSync(path.join(tmpdir(), 'slicc-shard-bootstrap-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  mkdirSync(path.join(directory, 'evidence'));
  const file = path.join(directory, 'evidence/bios-inputs.json');
  writeFileSync(file, JSON.stringify(value));
  const step = workflow.jobs.bios_inputs.steps.find((step) => step.id === 'inputs');
  const script = /node --input-type=module <<'NODE'\n([\s\S]*?)\nNODE/.exec(step.run)?.[1];
  assert.ok(script);
  const output = path.join(directory, 'output');
  execFileSync(process.execPath, ['--input-type=module', '-e', script], {
    cwd: directory,
    env: { ...process.env, GITHUB_OUTPUT: output },
  });
  return { document: JSON.parse(readFileSync(file)), output: readFileSync(output, 'utf8') };
}

test('one seven-shard JSON list matches the catalog at the exact reviewed BIOS ref', () => {
  assert.equal(pin.commit, snapshot.commit);
  assert.equal(pin.repository, snapshot.repository);
  assert.equal(
    createHash('sha1').update(`blob ${bytes.length}\0`).update(bytes).digest('hex'),
    snapshot.blob
  );
  const expected = Object.keys(catalog).filter(
    (name) => name === 'packages-update-cdp' || name.startsWith('guard-')
  );
  assert.deepEqual(pin.shards, expected);
  assert.deepEqual(biosShards, pin.shards);
  assert.equal(pin.shards.length, 7);
  assert.equal(pin.bindingCommit, 'c2cec127b935419049568f584093d7ef03ac0721');
  assert.equal(biosShardContract(pin, bytes).status, 'PASS');
  const names = artifactNames(plan);
  for (const side of ['base', 'candidate'])
    for (const shard of pin.shards)
      for (const kind of ['shard', 'files'])
        assert.equal(
          names[`bios-${side}-${kind}-${shard}`],
          `${side}-canary-${kind}-canary-${shard}`
        );
  assert.equal(names['bios-base-shard-guard'], undefined);
  for (const side of ['base', 'candidate']) {
    const job = workflow.jobs[`bios_${side}`];
    assert.equal(job.uses, `${pin.repository}/${pin.workflow}@${pin.commit}`);
    assert.equal(job.with.shards, '${{ needs.bios_inputs.outputs.shards }}');
    assert.equal(job.with['bios-ref'], '${{ needs.bios_inputs.outputs.bios_ref }}');
    assert.equal(job.with.repeat, 3);
  }
});

test('default calls keep existing legacy binding while bound calls retain staged strict IDs', () => {
  const subjects = Object.fromEntries(
    ['base', 'candidate'].map((side) => [
      side,
      { manifest: { version: '0.0.0' }, sha256: 'd'.repeat(64) },
    ])
  );
  const legacy = biosCalls(plan, pin, subjects);
  const bindings = {
    artifacts: ['base', 'candidate'].map((side, index) => ({
      role: `pack-${side}`,
      id: String(index + 1),
      digest: 'e'.repeat(64),
    })),
  };
  const strict = biosCalls(plan, selectBiosPin(pin, true), subjects, bindings);
  for (const side of ['base', 'candidate']) {
    assert.equal(legacy[side]['candidate-binding'], 'legacy');
    assert.equal(workflow.jobs[`bios_${side}`].with['candidate-binding'], 'legacy');
    assert.equal(legacy[side]['candidate-artifact-id'], undefined);
    assert.equal(strict[side]['candidate-binding'], 'strict');
    assert.equal(strict[side]['bios-ref'], pin.bindingCommit);
    assert.ok(strict[side]['candidate-artifact-id']);
    assert.equal(strict[side]['candidate-artifact-digest'], 'e'.repeat(64));
  }
});

for (const kind of ['unknown', 'missing', 'definition', 'duplicate', 'malformed'])
  test(`shard contract ${kind} is clear INCOMPLETE, never FAIL or PASS`, () => {
    const altered = structuredClone(catalog);
    const requested = structuredClone(pin);
    let document = bytes;
    if (kind === 'unknown') requested.shards.push('renamed-unknown');
    if (kind === 'missing') delete altered[pin.shards[1]];
    if (kind === 'definition') altered[pin.shards[1]] = null;
    if (kind === 'duplicate') requested.shards.push(pin.shards[0]);
    if (kind === 'malformed') document = Buffer.from('{');
    if (['missing', 'definition'].includes(kind)) document = Buffer.from(JSON.stringify(altered));
    const result = biosShardContract(requested, document);
    assert.equal(result.status, 'INCOMPLETE');
    assert.ok(result.incomplete.length);
    if (kind === 'unknown' || kind === 'missing')
      assert.match(result.incomplete.join(';'), /unknown BIOS shard .* at [a-f0-9]{40}/);
  });

test('catalog reads only the immutable pinned ref with no token or candidate execution', async () => {
  let called = 0;
  const result = await fetchBiosShardContract(pin, async (url, options) => {
    called++;
    assert.equal(
      url,
      `https://raw.githubusercontent.com/${pin.repository}/${pin.commit}/scripts/canary/shards.json`
    );
    assert.equal(options.headers, undefined);
    assert.equal(options.method, undefined);
    return new Response(bytes);
  });
  assert.equal(called, 1);
  assert.equal(result.status, 'PASS');
  assert.equal(result.commit, pin.commit);
  assert.equal(result.sha256, createHash('sha256').update(bytes).digest('hex'));
  assert.equal(
    (await fetchBiosShardContract(pin, async () => new Response('', { status: 404 }))).status,
    'INCOMPLETE'
  );
  assert.equal(
    (await fetchBiosShardContract(pin, async () => new Response(Buffer.alloc(65537)))).status,
    'INCOMPLETE'
  );
});

test('old MAIN bootstrap with old guard list is INCOMPLETE ready false and cannot launch producers', (t) => {
  const old = {
    status: 'PASS',
    ready: true,
    pin: { commit: '4025d6aabc2c06add216183064d81914a171cd64' },
    calls: { base: { shards: '["packages-update-cdp","guard"]' } },
  };
  const result = writer(old, t);
  assert.equal(result.document.status, 'INCOMPLETE');
  assert.equal(result.document.ready, false);
  assert.match(result.document.reason, /MAIN control has no pinned BIOS shard contract/);
  assert.match(result.output, /^ready=false\nshards=\nbios_ref=\n/);
  for (const side of ['base', 'candidate'])
    assert.ok(
      workflow.jobs[`bios_${side}`].if.includes("needs.bios_inputs.outputs.ready == 'true'")
    );
});

test('validated MAIN outputs transport the same list and pin to both producers', (t) => {
  const current = {
    status: 'PASS',
    ready: true,
    pin,
    shardContract: biosShardContract(pin, bytes),
    calls: {
      base: { shards: JSON.stringify(pin.shards) },
      candidate: { shards: JSON.stringify(pin.shards) },
    },
  };
  const result = writer(current, t);
  assert.equal(result.document.ready, true);
  assert.match(result.output, /^ready=true\n/);
  assert.ok(result.output.includes(`shards=${JSON.stringify(pin.shards)}\n`));
  assert.ok(result.output.includes(`bios_ref=${pin.commit}\n`));
});

function multiShardFixture() {
  const results = {};
  const executions = {};
  const flaky = new Set(pin.shards.filter((name) => name.startsWith('guard-')).slice(0, 2));
  let id = 100;
  for (const side of ['base', 'candidate']) {
    const tests = pin.shards.map((shard) => {
      const verdict = side === 'base' && flaky.has(shard) ? 'flaky' : 'pass';
      return {
        key: 'identical/full/ancestry/key',
        shard,
        test: 'same display name',
        verdict,
        failed: { candidate: verdict === 'flaky' ? '1/3' : '0/3', baseline: '0/0' },
      };
    });
    results[side] = {
      ok: side === 'candidate',
      repeat: 3,
      tests,
      counts: Object.fromEntries(
        ['pass', 'skip', 'fail', 'regression', 'both', 'flaky'].map((kind) => [
          kind,
          tests.filter((row) => row.verdict === kind).length,
        ])
      ),
      missingShards: [],
      unexplainedExits: [],
      ambiguous: [],
      refused: [],
      failures: [],
      regressions: [],
      both: [],
      flaky: tests
        .filter((row) => row.verdict === 'flaky')
        .map((row) => ({ ...row, log: 'retained first failure' })),
    };
    executions[side] = Object.fromEntries(
      tests.map((row) => {
        const artifactId = String(++id);
        return [
          row.shard,
          {
            artifact: {
              id: artifactId,
              name: `${side}-canary-shard-canary-${row.shard}`,
              url: `https://github.com/${plan.repository}/actions/runs/${plan.run}/artifacts/${artifactId}`,
              digest: 'd'.repeat(64),
              zipSha256: 'd'.repeat(64),
              documentSha256: 'e'.repeat(64),
              rowsSha256: 'f'.repeat(64),
            },
            document: {
              shard: row.shard,
              repeat: 3,
              exit: row.verdict === 'flaky' ? 1 : 0,
              results: [
                {
                  key: row.key,
                  status: row.verdict === 'flaky' ? 'fail' : 'pass',
                  runs:
                    row.verdict === 'flaky' ? ['pass', 'fail', 'pass'] : ['pass', 'pass', 'pass'],
                },
              ],
            },
          },
        ];
      })
    );
  }
  return { results, executions };
}

test('pairing and ledger retain multi-shard ancestry keys without collapsing equal display names', () => {
  const f = multiShardFixture();
  const paired = pairedBiosFindings(f.results, pin, {}, f.executions);
  assert.deepEqual(paired.failures, []);
  assert.deepEqual(paired.incomplete, []);
  assert.equal(paired.advisories.length, 2);
  assert.equal(new Set(paired.advisories.map((row) => row.shard)).size, 2);
  const ledger = biosAdvisoryLedger(plan, pin, paired.advisories, f.executions);
  const record = { ...plan, status: 'INCOMPLETE', bios: pin, biosLedger: ledger };
  assert.equal(biosLedgerObservations([record, structuredClone(record)]).length, 2);
  delete f.executions.candidate[pin.shards[1]];
  const missing = pairedBiosFindings(f.results, pin, {}, f.executions);
  assert.ok(missing.incomplete.length);
  assert.deepEqual(missing.advisories, []);
});
