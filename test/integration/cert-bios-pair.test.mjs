import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import test from 'node:test';

const file = new URL('../../scripts/cert/pr-bios-pair.mjs', import.meta.url);
const library = existsSync(file) ? await import(file.href) : null;
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
