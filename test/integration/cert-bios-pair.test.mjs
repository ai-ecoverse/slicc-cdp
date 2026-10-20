import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';

const file = new URL('../../scripts/cert/pr-bios-pair.mjs', import.meta.url);
const library = existsSync(file) ? await import(file.href) : null;
const ledgerFile = new URL('../../scripts/cert/pr-bios-ledger.mjs', import.meta.url);
const ledgerLibrary = existsSync(ledgerFile) ? await import(ledgerFile.href) : null;
const { biosCalls, biosFindings } = await import('../../scripts/cert/pr-bios-result.mjs');
const { validateBios } = await import('../../scripts/cert/pr-validate.mjs');
const rowsFile = new URL('../../scripts/cert/pr-bios-rows.mjs', import.meta.url);
const rowsLibrary = existsSync(rowsFile) ? await import(rowsFile.href) : null;
const pin = { shards: ['guard'] };
const kinds = ['pass', 'skip', 'fail', 'regression', 'both', 'flaky'];
const lists = { fail: 'failures', regression: 'regressions', both: 'both', flaky: 'flaky' };

function result(verdicts = ['pass', 'pass']) {
  const tests = verdicts.map((verdict, index) => ({
    key: `guard/ordinary/leaf-${index}`,
    shard: 'guard',
    test: `leaf ${index}`,
    verdict,
    failed: { candidate: verdict === 'pass' ? '0/3' : '3/3', baseline: '0/0' },
  }));
  const value = {
    biosRef: 'a'.repeat(40),
    run: 'canary',
    repeat: 3,
    url: 'https://github.com/fixture/lane/actions/runs/42',
    overrides: [],
    candidates: [],
    missingShards: [],
    unexplainedExits: [],
    refused: [],
    ambiguous: [],
    ok: true,
    tests,
    counts: Object.fromEntries(
      kinds.map((kind) => [kind, tests.filter((row) => row.verdict === kind).length])
    ),
  };
  for (const [kind, list] of Object.entries(lists))
    value[list] = tests
      .filter((row) => row.verdict === kind)
      .slice(0, 100)
      .map(({ key, shard, test: name, failed }) => ({
        key,
        shard,
        test: name,
        failed,
        log: 'actual failure',
      }));
  return value;
}

function report(value) {
  return {
    ...structuredClone(value),
    results: value.tests.map((row) => ({
      ...structuredClone(row),
      log: row.verdict === 'pass' ? '' : 'actual failure',
    })),
  };
}

function invoke(base, candidate, reports) {
  assert.ok(library, 'main-owned per-key bios comparator must exist');
  return library.pairedBiosFindings({ base, candidate }, pin, reports);
}
function actualFixture() {
  const base = result(['flaky', 'pass']);
  const candidate = result();
  const key = 'test/integration/guard-vectors.test.mjs › E2–E5, prompt detach › E2.vault control';
  for (const side of [base, candidate]) {
    side.tests[0].key = key;
    side.tests[0].test = 'guard-vectors.test.mjs › E2.vault control';
  }
  base.tests[0].failed.candidate = '1/3';
  base.flaky = [{ ...base.tests[0], log: 'actual failure' }];
  base.ok = false;
  const executions = Object.fromEntries(
    ['base', 'candidate'].map((side) => [
      side,
      {
        guard: {
          document: {
            shard: 'guard',
            repeat: 3,
            exit: side === 'base' ? 1 : 0,
            results: (side === 'base' ? base : candidate).tests.map((row, index) => ({
              key: row.key,
              status: side === 'base' && index === 0 ? 'fail' : 'pass',
              runs:
                side === 'base' && index === 0
                  ? ['pass', 'fail', 'pass']
                  : ['pass', 'pass', 'pass'],
            })),
          },
        },
      },
    ])
  );
  return { base, candidate, executions };
}

test('E2.vault control prompt baseline flake requires actual three candidate passes', () => {
  const f = actualFixture();
  const value = library.pairedBiosFindings(f, pin, {}, f.executions);
  assert.equal(status(value), 'PASS', JSON.stringify(value));
  assert.deepEqual(value.advisories, [
    {
      classification: 'base-only-flaky',
      shard: 'guard',
      key: f.base.tests[0].key,
      test: f.base.tests[0].test,
      base: { outcomes: ['pass', 'fail', 'pass'], rate: '1/3' },
      candidate: { outcomes: ['pass', 'pass', 'pass'], rate: '0/3' },
    },
  ]);
  assert.match(value.flags.join(';'), /base-only flaky advisory/);
  assert.match(value.flags.join(';'), /producer ok=false explained/);
});

for (const mode of [
  'two repeats',
  'skip',
  'timeout',
  'killed',
  'missing repeat',
  'missing row',
  'missing shard',
  'summary only',
  'candidate flaky',
  'wrong fraction',
  'duplicate row',
  'unexecuted status',
])
  test(`base-only advisory refuses ${mode}`, () => {
    const f = actualFixture();
    const row = f.executions.candidate.guard.document.results[0];
    if (mode === 'two repeats' || mode === 'missing repeat') row.runs.pop();
    if (mode === 'skip') row.runs[1] = 'skip';
    if (mode === 'timeout') row.runs[1] = 'timeout';
    if (mode === 'killed') f.executions.candidate.guard.document.signal = 'SIGKILL';
    if (mode === 'missing row') f.executions.candidate.guard.document.results.shift();
    if (mode === 'missing shard') delete f.executions.candidate.guard;
    if (mode === 'summary only') delete f.executions.candidate;
    if (mode === 'candidate flaky') {
      row.runs[1] = 'fail';
      f.candidate.tests[0].verdict = 'flaky';
      f.candidate.tests[0].failed.candidate = '1/3';
      f.candidate.counts.pass--;
      f.candidate.counts.flaky++;
      f.candidate.flaky = [{ ...f.candidate.tests[0], log: 'failure' }];
    }
    if (mode === 'wrong fraction') f.base.tests[0].failed.candidate = '2/3';
    if (mode === 'duplicate row')
      f.executions.candidate.guard.document.results.push(structuredClone(row));
    if (mode === 'unexecuted status') row.status = 'skip';
    const value = library.pairedBiosFindings(f, pin, {}, f.executions);
    assert.equal(status(value), 'INCOMPLETE', JSON.stringify(value));
    assert.deepEqual(value.advisories, []);
  });

test('missing candidate E2 key emits INCOMPLETE rather than throwing or recording an advisory', () => {
  const f = actualFixture();
  f.candidate.tests.shift();
  f.candidate.counts.pass--;
  f.executions.candidate.guard.document.results.shift();
  let value;
  assert.doesNotThrow(() => {
    value = library.pairedBiosFindings(f, pin, {}, f.executions);
  });
  assert.equal(status(value), 'INCOMPLETE');
  assert.match(value.incomplete.join(';'), /base\/candidate test row sets differ/);
  assert.deepEqual(value.advisories, []);
});

test('integrity FAIL retains precedence over a potential baseline advisory', () => {
  const f = actualFixture();
  const reports = { base: report(f.base), candidate: report(f.candidate) };
  reports.base.biosRef = 'f'.repeat(40);
  const value = library.pairedBiosFindings(f, pin, reports, f.executions);
  assert.equal(status(value), 'FAIL');
  assert.deepEqual(value.advisories, []);
});

function ledgerFixture() {
  const f = actualFixture();
  const plan = {
    repository: 'fixture/lane',
    run: '42',
    attempt: '1',
    pr: 11,
    head: 'b'.repeat(40),
    commit: 'c'.repeat(40),
  };
  const pinned = { ...pin, commit: 'a'.repeat(40) };
  const subjects = Object.fromEntries(
    ['base', 'candidate'].map((side, index) => [
      side,
      {
        manifest: { version: `0.0.0-${side}` },
        sha256: String(index + 1).repeat(64),
      },
    ])
  );
  const inventory = [];
  for (const [index, side] of ['base', 'candidate'].entries()) {
    const summary = f[side];
    summary.overrides = [
      {
        name: '@ai-ecoverse/slicc-cdp',
        version: subjects[side].manifest.version,
        sha256: subjects[side].sha256,
      },
    ];
    const artifact = `cert-pr-pack-${plan.pr}-${plan.head}-${plan.attempt}-${side}`;
    summary.candidates = [
      {
        package: `@ai-ecoverse/slicc-cdp@${subjects[side].manifest.version}`,
        via: 'artifact',
        status: `candidate (artifact ${plan.run}/${artifact}, sha256 ${subjects[side].sha256})`,
      },
    ];
    summary.artifact = { name: `${side}-canary`, url: `${summary.url}/artifacts/${10 + index}` };
    const document = f.executions[side].guard.document;
    document.pins = {
      overrides: [
        {
          ...summary.overrides[0],
          via: 'artifact',
          source: { artifact, run: plan.run, repo: plan.repository },
        },
      ],
    };
    const bytes = JSON.stringify(document);
    const digest = String(index + 3).repeat(64);
    f.executions[side].guard.bytes = bytes;
    f.executions[side].guard.artifact = {
      id: String(index + 20),
      name: `${side}-canary-shard-canary-guard`,
      digest,
      zipSha256: digest,
      documentSha256: createHash('sha256').update(bytes).digest('hex'),
      rowsSha256: createHash('sha256').update(bytes).digest('hex'),
      url: `${summary.url}/artifacts/${index + 20}`,
    };
    inventory.push({
      id: index + 20,
      name: `${side}-canary-shard-canary-guard`,
      digest: `sha256:${digest}`,
      expired: false,
      workflow_run: { id: 42, head_sha: plan.head },
    });
  }
  const bios = {
    status: 'PASS',
    pin: pinned,
    results: { base: f.base, candidate: f.candidate },
    rows: f.executions,
    calls: biosCalls(plan, pinned, subjects),
  };
  const context = { inventory, rows: structuredClone(f.executions) };
  Object.assign(bios, biosFindings(bios, pinned, plan, subjects, context));
  return { bios, plan, pinned, subjects, context };
}

test('producer advisory and independent record persist exact row provenance', () => {
  assert.ok(ledgerLibrary);
  const f = ledgerFixture();
  assert.deepEqual(f.bios.incomplete, []);
  assert.deepEqual(f.bios.failures, []);
  assert.equal(f.bios.biosLedger.length, 1);
  const emitted = [];
  const ledger = [];
  validateBios(
    f.bios,
    f.pinned,
    (status, reason) => emitted.push([status, reason]),
    f.plan,
    f.subjects,
    f.context,
    ledger
  );
  assert.deepEqual(emitted, []);
  assert.deepEqual(ledger, f.bios.biosLedger);
  const record = JSON.parse(
    JSON.stringify({ ...f.plan, status: 'PASS', bios: f.pinned, biosLedger: ledger })
  );
  const entries = ledgerLibrary.biosLedgerObservations([record, structuredClone(record)]);
  assert.equal(entries.length, 1);
  assert.deepEqual(entries[0].base, { outcomes: ['pass', 'fail', 'pass'], rate: '1/3' });
  assert.equal(entries[0].artifacts.candidate.id, '21');
  assert.equal(entries[0].audit, f.plan.commit);
  const next = structuredClone(record);
  next.run = '43';
  next.biosLedger[0].run = '43';
  for (const side of ['base', 'candidate'])
    next.biosLedger[0].artifacts[side].url = next.biosLedger[0].artifacts[side].url.replace(
      '/runs/42/',
      '/runs/43/'
    );
  assert.equal(ledgerLibrary.biosLedgerObservations([record, next]).length, 2);
  const conflict = structuredClone(record);
  conflict.biosLedger[0].artifacts.base.documentSha256 = 'f'.repeat(64);
  assert.throws(
    () => ledgerLibrary.biosLedgerObservations([record, conflict]),
    /conflicting duplicate/
  );
  const wrong = structuredClone(record);
  wrong.biosLedger[0].head = 'f'.repeat(40);
  assert.throws(() => ledgerLibrary.biosLedgerObservations([wrong]), /another record/);
  assert.throws(
    () => ledgerLibrary.biosLedgerObservations([{ ...record, status: 'CANCELLED' }]),
    /validated ledger/
  );
});

for (const mode of [
  'forged advisory',
  'forged ledger',
  'changed upload',
  'changed document bytes',
  'wrong pack source',
])
  test(`independent bios consumer rejects ${mode}`, () => {
    const f = ledgerFixture();
    if (mode === 'forged advisory') f.bios.advisories[0].base.rate = '2/3';
    if (mode === 'forged ledger') f.bios.biosLedger[0].run = '99';
    if (mode === 'changed upload') f.context.inventory[0].digest = `sha256:${'f'.repeat(64)}`;
    if (mode === 'changed document bytes')
      f.bios.rows.base.guard.document.results[0].runs[1] = 'pass';
    if (mode === 'wrong pack source') {
      const raw = f.bios.rows.base.guard;
      raw.document.pins.overrides[0].sha256 = 'f'.repeat(64);
      raw.bytes = JSON.stringify(raw.document);
      raw.artifact.documentSha256 = createHash('sha256').update(raw.bytes).digest('hex');
      raw.artifact.rowsSha256 = raw.artifact.documentSha256;
    }
    const emitted = [];
    validateBios(
      f.bios,
      f.pinned,
      (status, reason) => emitted.push([status, reason]),
      f.plan,
      f.subjects,
      f.context,
      []
    );
    assert.ok(
      emitted.some(([status]) => status === 'FAIL'),
      JSON.stringify(emitted)
    );
  });

test('historical first E2 observation retains original INCOMPLETE, never retroactive certification', () => {
  const seed = JSON.parse(
    readFileSync(new URL('../../scripts/cert/bios-ledger-first-case.json', import.meta.url))
  );
  assert.equal(seed.run, '39212646049');
  assert.equal(seed.originalStatus, 'INCOMPLETE');
  assert.equal(seed.originalCertified, false);
  assert.deepEqual(seed.base.outcomes, ['pass', 'fail', 'pass']);
  assert.equal(seed.base.rate, '1/3');
  assert.equal(seed.artifacts.base.id, '12177831767');
  assert.equal(seed.key, actualFixture().base.tests[0].key);
});

for (const mode of [
  'complete',
  'duplicate artifact',
  'missing artifact',
  'wrong ZIP digest',
  'wrong workflow head',
  'ambiguous document',
  'malformed document',
  'oversize stream',
])
  test(`actual uploaded BIOS row ingestion: ${mode}`, async (t) => {
    assert.ok(rowsLibrary);
    const directory = mkdtempSync(path.join(tmpdir(), 'slicc-ledger-test-'));
    t.after(() => rmSync(directory, { recursive: true, force: true }));
    const f = ledgerFixture();
    const pinned = { ...f.pinned, shards: ['guard', 'packages-update-cdp'] };
    const artifacts = [];
    const archives = new Map();
    for (const [index, side] of ['base', 'candidate'].entries())
      for (const [number, shard] of pinned.shards.entries()) {
        const document = structuredClone(f.bios.rows[side].guard.document);
        document.shard = shard;
        const text = mode === 'malformed document' ? 'not JSON' : JSON.stringify(document);
        writeFileSync(path.join(directory, 'shard.json'), text);
        const files = ['shard.json'];
        if (mode === 'ambiguous document') {
          const nested = path.join(directory, 'other');
          mkdirSync(nested, { recursive: true });
          writeFileSync(path.join(nested, 'shard.json'), text);
          files.push('other/shard.json');
        }
        const file = path.join(directory, `${side}-${shard}.zip`);
        execFileSync('zip', ['-q', file, ...files], { cwd: directory });
        const bytes = readFileSync(file);
        const id = index * 2 + number + 20;
        const digest = createHash('sha256').update(bytes).digest('hex');
        artifacts.push({
          id,
          name: `${side}-canary-shard-canary-${shard}`,
          digest: `sha256:${digest}`,
          expired: false,
          workflow_run: { id: 42, head_sha: f.plan.head },
        });
        archives.set(String(id), bytes);
      }
    if (mode === 'duplicate artifact') artifacts.push(structuredClone(artifacts[0]));
    if (mode === 'missing artifact') artifacts.shift();
    if (mode === 'wrong ZIP digest') artifacts[0].digest = `sha256:${'f'.repeat(64)}`;
    if (mode === 'wrong workflow head') artifacts[0].workflow_run.head_sha = 'f'.repeat(40);
    const original = globalThis.fetch;
    t.after(() => {
      globalThis.fetch = original;
    });
    let calls = 0;
    globalThis.fetch = async (url) => {
      calls++;
      const id = String(url).match(/artifacts\/(\d+)\/zip$/)?.[1];
      assert.ok(archives.has(id));
      if (mode === 'oversize stream')
        return new Response(
          new ReadableStream({
            start(controller) {
              for (let i = 0; i < 33; i++) controller.enqueue(new Uint8Array(1024 * 1024));
              controller.close();
            },
          })
        );
      return new Response(archives.get(id));
    };
    const api = async (route) => {
      assert.match(route, /^actions\/runs\/42\/artifacts\?per_page=100&page=1$/);
      return { total_count: artifacts.length, artifacts };
    };
    if (mode === 'complete') {
      const actual = await rowsLibrary.biosExecutionRows(f.plan, pinned, api);
      assert.equal(calls, 4);
      assert.deepEqual(Object.keys(actual.base), pinned.shards);
      assert.equal(actual.candidate.guard.artifact.zipSha256, artifacts[2].digest.slice(7));
      assert.deepEqual(actual.base.guard.document.results[0].runs, ['pass', 'fail', 'pass']);
    } else {
      const error = await rowsLibrary.biosExecutionRows(f.plan, pinned, api).then(
        () => null,
        (error) => error
      );
      assert.ok(error, mode);
      if (mode === 'wrong ZIP digest') assert.equal(error.certStatus, 'FAIL');
    }
  });

const status = (value) =>
  value.failures.length ? 'FAIL' : value.incomplete.length ? 'INCOMPLETE' : 'PASS';

test('equal counts do not excuse a wholly unexecuted planned shard', () => {
  assert.ok(library, 'main-owned per-key bios comparator must exist');
  const value = library.pairedBiosFindings(
    { base: result(), candidate: result() },
    {
      shards: ['guard', 'packages-update-cdp'],
    }
  );
  assert.equal(status(value), 'INCOMPLETE');
  assert.match(value.incomplete.join(';'), /unexecuted planned shard/);
});

test('a stable node key may appear once in each distinct shard', () => {
  assert.ok(library, 'main-owned per-key bios comparator must exist');
  const base = result();
  const candidate = result();
  for (const value of [base, candidate]) {
    value.tests[1].shard = 'packages-update-cdp';
    value.tests[1].key = value.tests[0].key;
    value.tests[1].test = value.tests[0].test;
  }
  const value = library.pairedBiosFindings(
    { base, candidate },
    { shards: ['guard', 'packages-update-cdp'] }
  );
  assert.equal(status(value), 'PASS', JSON.stringify(value));
});

const cases = [
  [
    'reported ambiguity cannot be explained away by shared failures',
    (f) => {
      f.base = result(['regression', 'pass']);
      f.candidate = result(['regression', 'pass']);
      f.candidate.ok = false;
      f.candidate.ambiguous = [{ shard: 'guard', key: 'hidden collision', rows: 2 }];
    },
    'INCOMPLETE',
  ],
  [
    'missing ambiguity evidence is incomplete',
    (f) => {
      delete f.base.ambiguous;
    },
    'INCOMPLETE',
  ],
  [
    'uncapped 110-row evidence survives capped diagnostic lists',
    (f) => {
      f.base = result(Array(110).fill('regression'));
      f.candidate = result(Array(110).fill('regression'));
      f.base.ok = false;
      f.candidate.ok = false;
    },
    'PASS',
  ],
  [
    '100-row diagnostic cap cannot hide truncated evidence',
    (f) => {
      f.base = result(Array(110).fill('regression'));
      f.candidate = result(Array(110).fill('regression'));
      f.candidate.tests = f.candidate.tests.slice(0, 100);
    },
    'INCOMPLETE',
  ],
  [
    'duplicate display titles require distinct stable keys',
    (f) => {
      f.base.tests[1].test = f.base.tests[0].test;
      f.candidate.tests[1].test = f.candidate.tests[0].test;
    },
    'PASS',
  ],
  ['all complete keyed tests pass', () => {}, 'PASS'],
  [
    'actual candidate failure with base PASS',
    (f) => {
      f.candidate = result(['regression', 'pass']);
    },
    'FAIL',
  ],
  [
    'candidate both-versus-pins still introduces failure against PR base',
    (f) => {
      f.candidate = result(['both', 'pass']);
    },
    'FAIL',
  ],
  [
    'same failure on both packs is a flag',
    (f) => {
      f.base = result(['regression', 'pass']);
      f.candidate = result(['regression', 'pass']);
    },
    'PASS',
  ],
  [
    'all tests failing on both is not missing execution',
    (f) => {
      f.base = result(['regression', 'fail']);
      f.candidate = result(['regression', 'both']);
      f.base.ok = false;
      f.candidate.ok = false;
    },
    'PASS',
  ],
  [
    'different bios-pin failure categories on the same PR key',
    (f) => {
      f.base = result(['both', 'pass']);
      f.candidate = result(['regression', 'pass']);
    },
    'PASS',
  ],
  [
    'candidate improvement is not a new failure',
    (f) => {
      f.base = result(['regression', 'pass']);
    },
    'PASS',
  ],
  [
    'producer false wholly explained by shared failures',
    (f) => {
      f.base = result(['regression', 'pass']);
      f.candidate = result(['regression', 'pass']);
      f.base.ok = false;
      f.candidate.ok = false;
    },
    'PASS',
  ],
  [
    'producer false without failed rows',
    (f) => {
      f.candidate.ok = false;
    },
    'INCOMPLETE',
  ],
  [
    'base producer false explained by candidate improvement',
    (f) => {
      f.base = result(['regression', 'pass']);
      f.base.ok = false;
    },
    'PASS',
  ],
  [
    'missing base tests',
    (f) => {
      delete f.base.tests;
    },
    'INCOMPLETE',
  ],
  [
    'missing candidate tests',
    (f) => {
      delete f.candidate.tests;
    },
    'INCOMPLETE',
  ],
  [
    'truncated tests cannot hide counts',
    (f) => {
      f.candidate.tests.pop();
    },
    'INCOMPLETE',
  ],
  [
    'equal counts but different stable key sets',
    (f) => {
      f.candidate.tests[0].key = 'another-key';
    },
    'INCOMPLETE',
  ],
  [
    'same stable key but different test identity',
    (f) => {
      f.candidate.tests[0].test = 'another-title';
    },
    'INCOMPLETE',
  ],
  [
    'missing stable key is not an occurrence fallback',
    (f) => {
      delete f.candidate.tests[0].key;
    },
    'INCOMPLETE',
  ],
  [
    'duplicate stable keys',
    (f) => {
      f.candidate.tests[1].key = f.candidate.tests[0].key;
    },
    'INCOMPLETE',
  ],
  [
    'partial actual repeat',
    (f) => {
      f.candidate.tests[0].failed.candidate = '0/2';
    },
    'INCOMPLETE',
  ],
  [
    'empty actual repeat',
    (f) => {
      f.candidate.tests[0].failed.candidate = '0/0';
    },
    'INCOMPLETE',
  ],
  [
    'missing completed-run denominator',
    (f) => {
      delete f.candidate.tests[0].failed;
    },
    'INCOMPLETE',
  ],
  [
    'pass label contradicts completed failures',
    (f) => {
      f.candidate.tests[0].failed.candidate = '1/3';
    },
    'INCOMPLETE',
  ],
  [
    'skip is not pass',
    (f) => {
      f.candidate = result(['skip', 'pass']);
    },
    'INCOMPLETE',
  ],
  [
    'flaky is not pass',
    (f) => {
      f.candidate = result(['flaky', 'pass']);
    },
    'INCOMPLETE',
  ],
  [
    'missing shards prevent producer false waiver',
    (f) => {
      f.base = result(['regression', 'pass']);
      f.candidate = result(['regression', 'pass']);
      f.candidate.ok = false;
      f.candidate.missingShards = ['guard'];
    },
    'INCOMPLETE',
  ],
  [
    'artifact refusal cannot be waived',
    (f) => {
      f.base = result(['regression', 'pass']);
      f.candidate = result(['regression', 'pass']);
      f.candidate.refused = [{ name: 'forged' }];
    },
    'INCOMPLETE',
  ],
  [
    'refusal evidence must be present',
    (f) => {
      delete f.candidate.refused;
    },
    'INCOMPLETE',
  ],
  [
    'unexplained exit cannot be waived',
    (f) => {
      f.candidate.unexplainedExits = [{ shard: 'guard', exit: 1 }];
    },
    'INCOMPLETE',
  ],
  [
    'bad summary counts',
    (f) => {
      f.candidate.counts.pass = '2';
    },
    'INCOMPLETE',
  ],
  [
    'summary capped list disagrees with full keyed rows',
    (f) => {
      f.candidate = result(['regression', 'pass']);
      f.candidate.regressions = [];
    },
    'INCOMPLETE',
  ],
  [
    'null test row',
    (f) => {
      f.candidate.tests[0] = null;
    },
    'INCOMPLETE',
  ],
  [
    'empty row set',
    (f) => {
      f.base.tests = [];
    },
    'INCOMPLETE',
  ],
];

for (const [name, mutate, expected] of cases)
  test(`bios PR-pack pairing: ${name}`, () => {
    const f = { base: result(), candidate: result() };
    mutate(f);
    const value = invoke(f.base, f.candidate);
    assert.equal(status(value), expected, JSON.stringify(value));
    if (name.includes('improvement')) assert.match(value.flags.join(';'), /improved by candidate/);
    if (name.includes('both packs') || name.includes('wholly explained'))
      assert.match(value.flags.join(';'), /pre-existing/);
  });

for (const mode of [
  'complete',
  'wrong source',
  'truncated',
  'wrong counts',
  'wrong list log',
  'wrong candidate identity',
  'split row sets',
])
  test(`bound full canary row pairing: ${mode}`, () => {
    const base = result();
    const candidate = result(['regression', 'pass']);
    const full = { base: report(base), candidate: report(candidate) };
    if (mode === 'wrong source') full.base.url += '-forged';
    if (mode === 'truncated') full.base.results.pop();
    if (mode === 'wrong counts') full.candidate.counts.pass = 0;
    if (mode === 'wrong list log') full.candidate.results[0].log = 'different failure';
    if (mode === 'wrong candidate identity')
      full.candidate.candidates = [{ package: 'forged', via: 'artifact', status: 'forged' }];
    if (mode === 'split row sets') full.base.results[0].key = 'different';
    if (mode !== 'split row sets') {
      delete base.tests;
      delete candidate.tests;
    }
    const value = invoke(base, candidate, full);
    assert.equal(
      status(value),
      ['truncated', 'split row sets'].includes(mode) ? 'INCOMPLETE' : 'FAIL',
      JSON.stringify(value)
    );
    if (mode === 'complete') assert.match(value.failures.join(';'), /candidate introduced failure/);
  });
