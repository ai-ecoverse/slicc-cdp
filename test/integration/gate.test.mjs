import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const gate = path.join(root, 'scripts/gate.mjs');
const env = { ...process.env };
delete env.NODE_TEST_CONTEXT;

function gated(args, script) {
  return spawnSync(process.execPath, [gate, ...args, '--', process.execPath, '-e', script], {
    encoding: 'utf8',
    env,
  });
}

test('a gate fails when its script is silent, exits early, or names another verdict', () => {
  for (const [args, script, why] of [
    [['demo'], '', 'empty output'],
    [['demo'], 'console.log("done"); process.exit(0)', 'early exit 0 without a verdict'],
    [['demo'], 'console.log("other: PASS 3 cases")', 'a different gate'],
    [['demo'], 'console.log("demo: FAIL 2 of 3")', 'a FAIL verdict'],
    [['demo'], 'console.log("demo: PASSED")', 'a near miss'],
    [['demo'], 'console.log("demo: PASS 3 cases"); process.exit(1)', 'a non-zero exit'],
    [['demo', '--tap'], 'console.log("# tests 0\\n# pass 0\\n# fail 0")', 'no test ran'],
    [['demo', '--tap'], 'console.log("ok 1 - a\\n# pass 2\\n# fail 1")', 'a failed test'],
    [
      ['demo', '--tap'],
      'console.log("ok 1 - /t/empty.test.mjs\\n# pass 1\\n# fail 0")',
      'only an empty file',
    ],
    [['demo', '--tap'], 'console.log("ok 1 - a # SKIP\\n# pass 0\\n# fail 0")', 'only skips'],
    [['demo', '--tap'], '', 'no TAP summary'],
  ]) {
    const result = gated(args, script);
    assert.equal(result.status, 1, `${why} passed: ${result.stdout}`);
    assert.match(result.stderr, /^demo: FAIL /m, why);
  }
});

test('a gate passes on its own verdict line, and --accept widens the verdicts', () => {
  const pass = gated(['demo'], 'console.log("work\\ndemo: PASS 3 cases")');
  assert.equal(pass.status, 0, pass.stderr);
  assert.match(pass.stdout, /^demo: PASS 3 cases$/m);
  const evidence = gated(
    ['demo', '--accept', 'PASS,FAIL,INCOMPLETE'],
    'console.log("demo: FAIL x.json")'
  );
  assert.equal(evidence.status, 0, evidence.stderr);
  const tap = gated(
    ['demo', '--tap'],
    'console.log("ok 1 - a\\nok 2 - b\\n# tests 2\\n# pass 2\\n# fail 0")'
  );
  assert.equal(tap.status, 0, tap.stderr);
  assert.match(tap.stdout, /^demo: PASS 2 tests$/m);
});

test('the integration runner fails when no test runs', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cdp-gate-'));
  try {
    const empty = path.join(dir, 'empty.test.mjs');
    writeFileSync(empty, '');
    const runner = path.join(root, 'scripts/test-integration.mjs');
    const none = spawnSync(
      process.execPath,
      [gate, 'integration', '--', process.execPath, runner, empty],
      {
        encoding: 'utf8',
        env,
      }
    );
    assert.equal(none.status, 1, none.stdout);
    assert.match(none.stderr, /^integration: FAIL no test ran$/m);
    const one = path.join(dir, 'one.test.mjs');
    writeFileSync(one, "import test from 'node:test';\ntest('one', () => {});\n");
    const ran = spawnSync(
      process.execPath,
      [gate, 'integration', '--', process.execPath, runner, one],
      {
        encoding: 'utf8',
        env,
      }
    );
    assert.equal(ran.status, 0, ran.stderr);
    assert.match(ran.stdout, /^integration: PASS 1 tests$/m);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('every gate script a workflow runs goes through gate.mjs and prints its verdict', () => {
  const dir = path.join(root, '.github/workflows');
  const ungated = [];
  const names = new Map();
  for (const file of readdirSync(dir).filter((name) => name.endsWith('.yml'))) {
    for (const line of readFileSync(path.join(dir, file), 'utf8').split('\n')) {
      const script = line.match(/(?:node|sh|bash) (scripts\/[\w/.-]+\.(?:mjs|sh))/)?.[1];
      const runsTests = /node --test/.test(line) && !/negative\.log/.test(line);
      if (!script && !runsTests && !/npm run test:integration/.test(line)) continue;
      if (script === 'scripts/sign-macos.sh') continue;
      const gate = line.match(/node scripts\/gate\.mjs "?([\w$-]+)"?/);
      if (!gate) {
        ungated.push(`${file}: ${line.trim()}`);
        continue;
      }
      const target = line.split(' -- ')[1]?.match(/(scripts\/[\w/.-]+\.(?:mjs|sh))/)?.[1];
      if (target && !line.includes('--tap')) names.set(target, gate[1]);
    }
  }
  assert.deepEqual(ungated, []);
  for (const [script, name] of names) {
    const text = readFileSync(path.join(root, script), 'utf8');
    const source =
      script.startsWith('scripts/cert/') && /await evidence\(/.test(text)
        ? readFileSync(path.join(root, 'scripts/cert/common.mjs'), 'utf8')
        : text;
    const literal = name.startsWith('assets-') || source !== text ? '${which}: ' : `${name}: `;
    assert.ok(source.includes(literal), `${script} prints no "${literal}" verdict`);
  }
  assert.ok(names.size >= 15, [...names.keys()].join(', '));
});
