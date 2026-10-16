import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));

function execute(dir, file) {
  const out = path.join(dir, 'result');
  const process = spawnSync(
    'node',
    [
      path.join(root, 'scripts/cert/run-suite.mjs'),
      '--directory',
      dir,
      '--files',
      JSON.stringify([file]),
      '--out',
      out,
    ],
    { encoding: 'utf8', timeout: 30000 }
  );
  return { process, result: JSON.parse(readFileSync(`${out}.json`, 'utf8')), out };
}

test('cert suite counts an entry wrapper whose behavioural tests are defined in an imported helper', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cert-wrapper-'));
  try {
    writeFileSync(path.join(dir, 'entry.test.mjs'), 'import "./helper.mjs";\n');
    writeFileSync(
      path.join(dir, 'helper.mjs'),
      'import test from "node:test"; test("public behaviour",()=>{});\n'
    );
    const { process, result } = execute(dir, 'entry.test.mjs');
    assert.equal(process.status, 0, process.stderr);
    assert.equal(result.status, 'PASS');
    assert.deepEqual(result.missing, []);
    assert.equal(result.attempts.length, 1);
    assert.ok(
      result.rows.some((row) => row.file?.endsWith('helper.mjs') && row.type === 'test:pass')
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('cert suite keeps a module-load failure diagnostic instead of an empty stderr log', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cert-diagnostic-'));
  try {
    writeFileSync(path.join(dir, 'entry.test.mjs'), 'import "./missing.mjs";\n');
    const { process, result, out } = execute(dir, 'entry.test.mjs');
    assert.equal(process.status, 1);
    assert.equal(result.status, 'INCOMPLETE');
    assert.match(readFileSync(`${out}.1.log`, 'utf8'), /ERR_MODULE_NOT_FOUND/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('the current browser fixture loads independently of an older subject without the browser API', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cert-fixture-'));
  try {
    const helpers = path.join(dir, 'test/integration');
    mkdirSync(helpers, { recursive: true });
    const file = path.join(helpers, 'browser.mjs');
    copyFileSync(path.join(root, 'test/integration/browser.mjs'), file);
    const outcome = spawnSync(
      'node',
      [
        '--input-type=module',
        '-e',
        `const fixture = await import(${JSON.stringify(pathToFileURL(file).href)}); console.log(typeof fixture.launchBrowser);`,
      ],
      {
        encoding: 'utf8',
        env: {
          ...process.env,
          SLICC_FIXTURE_BROWSER_API: pathToFileURL(path.join(root, 'src/browser.js')).href,
        },
      }
    );
    assert.equal(outcome.status, 0, outcome.stderr);
    assert.equal(outcome.stdout.trim(), 'function');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('workflow-call proof CLI requires execution despite a skipped caller ancestor and rejects missing jobs', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cert-call-'));
  try {
    const record = path.join(dir, 'record.json');
    const jobs = path.join(dir, 'jobs.json');
    writeFileSync(record, JSON.stringify({ run: '123', version: '1.26.2', status: 'INCOMPLETE' }));
    const required = [
      'plan',
      'rebuild',
      'guard',
      'integration',
      'assets (ubuntu-latest)',
      'assets (macos-latest)',
      'record',
    ];
    const data = {
      jobs: required.map((name) => ({
        name: `cert / ${name}`,
        status: 'completed',
        conclusion: 'success',
      })),
    };
    const run = () =>
      spawnSync(
        'node',
        [path.join(root, 'scripts/cert/verify-call.mjs'), '--record', record, '--jobs', jobs],
        {
          encoding: 'utf8',
          env: { ...process.env, GITHUB_RUN_ID: '123', UPSTREAM: 'skipped' },
        }
      );
    writeFileSync(jobs, JSON.stringify(data));
    assert.equal(run().status, 0);
    for (const state of ['skipped', 'cancelled']) {
      data.jobs[1].conclusion = state;
      writeFileSync(jobs, JSON.stringify(data));
      const rejected = run();
      assert.equal(rejected.status, 1);
      assert.match(rejected.stderr, /required job: rebuild/);
    }
    data.jobs.splice(1, 1);
    writeFileSync(jobs, JSON.stringify(data));
    assert.equal(run().status, 1);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
