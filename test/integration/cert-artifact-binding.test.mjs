import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const libraryFile = new URL('../../scripts/cert/pr-artifact-binding.mjs', import.meta.url);
const library = existsSync(libraryFile) ? await import(libraryFile.href) : null;
const invokeLibrary = (name, args) => {
  assert.ok(library, 'main-owned artifact binding verifier must exist');
  return library[name](...args);
};
const artifactDigest = (...args) => invokeLibrary('artifactDigest', args);
const artifactFindings = (...args) => invokeLibrary('artifactFindings', args);
const artifactInventory = (...args) => invokeLibrary('artifactInventory', args);
const artifactNames = (...args) => invokeLibrary('artifactNames', args);

import { prIdentity } from '../../scripts/cert/pr-common.mjs';
import { assertVerdict } from './cert-cli-verdict.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const bytes = Buffer.from('opaque archive fixture, never extracted or executed');
const sha = createHash('sha256').update(bytes).digest('hex');
const plan = {
  mode: 'pr',
  go: true,
  pr: 273,
  repository: 'fixture/lane',
  run: '42',
  attempt: '1',
  head: 'a'.repeat(40),
  commit: 'b'.repeat(40),
  tools: 'c'.repeat(40),
  version: '0.0.0-pr.273.42',
  baseline: { version: '0.0.0-base.273.42', commit: 'c'.repeat(40) },
};
function fixture() {
  const names = artifactNames(plan);
  const bindings = {
    schema: 1,
    ...prIdentity(plan),
    artifacts: Object.entries(names).map(([role, name], index) => ({
      role,
      name,
      id: String(index + 1),
      digest: sha,
    })),
  };
  const actual = bindings.artifacts.map((row) => ({
    id: Number(row.id),
    name: row.name,
    digest: `sha256:${sha}`,
    expired: false,
    workflow_run: { id: Number(plan.run), head_sha: plan.head },
  }));
  return { names, bindings, actual };
}
const cases = [
  ['exact producer ID/digest and API identity', () => {}, 'PASS'],
  [
    'forged extra evidence suffix',
    (f) =>
      f.actual.push({
        ...f.actual[0],
        id: 90,
        name: `cert-pr-evidence-${plan.run}-${plan.attempt}-forged`,
      }),
    'FAIL',
  ],
  [
    'extra bios shard suffix',
    (f) => f.actual.push({ ...f.actual[0], id: 90, name: 'base-canary-shard-canary-forged' }),
    'FAIL',
  ],
  ['duplicate exact name', (f) => f.actual.push({ ...f.actual[0], id: 90 }), 'FAIL'],
  [
    'duplicate API ID',
    (f) => {
      f.actual[1].id = f.actual[0].id;
    },
    'FAIL',
  ],
  [
    'wrong producer ID',
    (f) => {
      f.bindings.artifacts.find((row) => row.role === 'guard').id = '91';
    },
    'FAIL',
  ],
  [
    'wrong producer digest',
    (f) => {
      f.bindings.artifacts.find((row) => row.role === 'guard').digest = 'd'.repeat(64);
    },
    'FAIL',
  ],
  [
    'same public identity does not justify renamed producer artifact',
    (f) => {
      f.bindings.artifacts.find((row) => row.role === 'guard').name += '-forged';
    },
    'FAIL',
  ],
  [
    'bindings for another head',
    (f) => {
      f.bindings.head = 'd'.repeat(40);
    },
    'FAIL',
  ],
  [
    'different run',
    (f) => {
      f.actual.find((row) => row.name === f.names.guard).workflow_run.id = 43;
    },
    'FAIL',
  ],
  [
    'different API head',
    (f) => {
      f.actual.find((row) => row.name === f.names.guard).workflow_run.head_sha = 'd'.repeat(40);
    },
    'FAIL',
  ],
  [
    'expired selected artifact',
    (f) => {
      f.actual.find((row) => row.name === f.names.guard).expired = true;
    },
    'INCOMPLETE',
  ],
  [
    'missing producer digest',
    (f) => {
      delete f.bindings.artifacts.find((row) => row.role === 'guard').digest;
    },
    'INCOMPLETE',
  ],
  [
    'malformed producer digest',
    (f) => {
      f.bindings.artifacts.find((row) => row.role === 'guard').digest = 'sha512:bad';
    },
    'FAIL',
  ],
  [
    'zero producer ID',
    (f) => {
      f.bindings.artifacts.find((row) => row.role === 'guard').id = 0;
    },
    'FAIL',
  ],
  [
    'missing API digest',
    (f) => {
      delete f.actual.find((row) => row.name === f.names.guard).digest;
    },
    'INCOMPLETE',
  ],
  [
    'missing API run',
    (f) => {
      delete f.actual.find((row) => row.name === f.names.guard).workflow_run;
    },
    'INCOMPLETE',
  ],
  [
    'missing selected artifact',
    (f) => {
      f.actual = f.actual.filter((row) => row.name !== f.names.guard);
    },
    'INCOMPLETE',
  ],
  [
    'missing bindings',
    (f) => {
      f.bindings = null;
    },
    'INCOMPLETE',
  ],
  [
    'extra artifact wins over missing bindings',
    (f) => {
      f.bindings = null;
      f.actual.push({ id: 90, name: 'forged' });
    },
    'FAIL',
  ],
];
for (const [name, change, status] of cases) {
  test(`artifact binding: ${name}`, () => {
    const f = fixture();
    change(f);
    const result = artifactFindings(plan, f.bindings, f.actual, { roles: ['guard'] });
    assert.equal(result.status, status, JSON.stringify(result));
    if (status === 'PASS')
      assert.deepEqual(result.matched, [
        {
          role: 'guard',
          name: f.names.guard,
          id: f.bindings.artifacts.find((row) => row.role === 'guard').id,
          digest: sha,
        },
      ]);
  });
}

test('record and report require every produced binding, but not their own future upload', () => {
  const f = fixture();
  const shardCount = JSON.parse(
    readFileSync(new URL('../../scripts/cert/pr-bios.json', import.meta.url))
  ).shards.length;
  assert.equal(Object.keys(f.names).length, 24 + 4 * shardCount);
  f.actual = f.actual.filter((row) => ![f.names.record, f.names.report].includes(row.name));
  assert.equal(artifactFindings(plan, f.bindings, f.actual, { phase: 'record' }).status, 'PASS');
  assert.equal(
    artifactFindings(plan, f.bindings, f.actual, { phase: 'report' }).status,
    'INCOMPLETE'
  );
  f.actual.push({
    id: Number(f.bindings.artifacts.find((row) => row.role === 'record').id),
    name: f.names.record,
    digest: sha,
    expired: false,
    workflow_run: { id: 42, head_sha: plan.head },
  });
  assert.equal(artifactFindings(plan, f.bindings, f.actual, { phase: 'report' }).status, 'PASS');
  const pin = JSON.parse(readFileSync(new URL('../../scripts/cert/pr-bios.json', import.meta.url)));
  f.bindings.artifacts = f.bindings.artifacts.filter(
    (row) => row.role !== `bios-base-shard-${pin.shards[1]}`
  );
  assert.equal(
    artifactFindings(plan, f.bindings, f.actual, { phase: 'report' }).status,
    'INCOMPLETE'
  );
});

test('empty scopes, wildcard roles and unknown phases cannot assert producer verification', () => {
  const f = fixture();
  assert.equal(artifactFindings(plan, f.bindings, f.actual).status, 'INCOMPLETE');
  for (const options of [
    { roles: ['guard*'] },
    { phase: 'unverified' },
    { roles: ['guard', 'guard'] },
  ])
    assert.equal(artifactFindings(plan, f.bindings, f.actual, options).status, 'FAIL');
  assert.equal(artifactDigest(`sha256:${sha}`), sha);
  assert.equal(artifactDigest(sha), sha);
  assert.equal(artifactDigest('sha512:' + sha), null);
});

test('complete API inventory paginates and refuses truncated or changing snapshots', async () => {
  const rows = Array.from({ length: 101 }, (_, id) => ({ id }));
  const calls = [];
  const actual = await artifactInventory(async (route) => {
    calls.push(route);
    return {
      total_count: 101,
      artifacts: route.endsWith('page=1') ? rows.slice(0, 100) : rows.slice(100),
    };
  }, '42');
  assert.deepEqual(actual, rows);
  assert.equal(calls.length, 2);
  await assert.rejects(
    artifactInventory(async () => ({ total_count: 101, artifacts: rows.slice(0, 99) }), '42'),
    /truncated/
  );
  let page = 0;
  await assert.rejects(
    artifactInventory(
      async () => ({
        total_count: ++page === 1 ? 101 : 102,
        artifacts: page === 1 ? rows.slice(0, 100) : rows.slice(100),
      }),
      '42'
    ),
    /changing/
  );
});

async function cliFixture(t, change = () => {}) {
  const f = fixture();
  change(f);
  const directory = mkdtempSync(path.join(tmpdir(), 'slicc-artifact-binding-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  for (const [name, value] of [
    ['plan', plan],
    ['bindings', f.bindings],
  ])
    writeFileSync(path.join(directory, `${name}.json`), JSON.stringify(value));
  const calls = [];
  const server = createServer((req, res) => {
    calls.push(req.url);
    if (req.url.endsWith('/zip')) {
      res.end(f.zipBytes ?? bytes);
      return;
    }
    res.setHeader('content-type', 'application/json');
    res.end(JSON.stringify({ total_count: f.actual.length, artifacts: f.actual }));
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(
    () =>
      new Promise((resolve) => {
        server.closeAllConnections();
        server.close(resolve);
      })
  );
  const invoke = async (script, argv) => {
    const legacy = script === 'pr-record.mjs' ? process.env.SLICC_ARTIFACT_RECORD_SOURCE : null;
    const bindingIndex = argv.indexOf('--bindings');
    const compatible =
      legacy && bindingIndex >= 0
        ? argv.filter((_, index) => index !== bindingIndex && index !== bindingIndex + 1)
        : argv;
    const child = spawn(
      process.execPath,
      [legacy ?? path.join(root, 'scripts/cert', script), ...compatible],
      {
        cwd: directory,
        env: {
          ...process.env,
          GITHUB_API_URL: `http://127.0.0.1:${server.address().port}`,
          GH_TOKEN: '',
          GITHUB_OUTPUT: '',
          GITHUB_STEP_SUMMARY: '',
          CERT_PR_PACKS: '',
          CERT_PR_ASSETS: '',
          SLICC_CDP_PORTS: '9300-9309',
        },
      }
    );
    let stdout = '',
      stderr = '';
    child.stdout.on('data', (data) => {
      stdout += data;
    });
    child.stderr.on('data', (data) => {
      stderr += data;
    });
    const exit = await new Promise((resolve, reject) => {
      child.once('error', reject);
      child.once('close', resolve);
    });
    return { exit, stdout, stderr };
  };
  return { ...f, directory, calls, invoke };
}
for (const [name, change, expected] of [
  ['verified bytes', () => {}, 'PASS'],
  [
    'wrong ZIP bytes despite matching API digest',
    (f) => {
      f.zipBytes = Buffer.from('tampered');
    },
    'FAIL',
  ],
  [
    'forged extra suffix prevents any download',
    (f) => f.actual.push({ id: 90, name: 'cert-pr-evidence-42-1-forged' }),
    'FAIL',
  ],
]) {
  test(`artifact binding public CLI: ${name}`, async (t) => {
    const f = await cliFixture(t, change);
    const out = path.join(f.directory, 'result.json');
    const target = path.join(f.directory, 'archives');
    const run = await f.invoke('pr-artifacts.mjs', [
      '--plan',
      path.join(f.directory, 'plan.json'),
      '--bindings',
      path.join(f.directory, 'bindings.json'),
      '--roles',
      '["guard"]',
      '--action',
      'download',
      '--directory',
      target,
      '--out',
      out,
    ]);
    const result = JSON.parse(readFileSync(out));
    assert.equal(result.status, expected);
    assert.equal(run.exit, expected === 'PASS' ? 0 : 1, run.stderr);
    assertVerdict(run.stdout, 'pr-artifacts', expected);
    assert.equal(existsSync(path.join(target, 'guard.zip')), expected === 'PASS');
    assert.equal(
      f.calls.filter((url) => url.endsWith('/zip')).length,
      name.includes('extra suffix') ? 0 : 1
    );
    if (expected === 'PASS') assert.deepEqual(readFileSync(path.join(target, 'guard.zip')), bytes);
  });
}

test('record public CLI FAILs a forged suffix despite otherwise missing evidence', async (t) => {
  const f = await cliFixture(t, (f) =>
    f.actual.push({ id: 90, name: 'cert-pr-evidence-42-1-forged' })
  );
  const out = path.join(f.directory, 'record');
  const run = await f.invoke('pr-record.mjs', [
    '--evidence',
    f.directory,
    '--bindings',
    path.join(f.directory, 'bindings.json'),
    '--out',
    out,
  ]);
  const record = JSON.parse(readFileSync(path.join(out, 'record.json')));
  assert.equal(record.status, 'FAIL');
  assert.equal(record.certified, false);
  assert.ok(
    record.failures.includes('artifacts: unexpected run artifact: cert-pr-evidence-42-1-forged')
  );
  assert.ok(record.incomplete.length > 0);
  assert.equal(run.exit, 0, run.stderr);
  assertVerdict(run.stdout, 'pr-record', 'FAIL');
});

test('inspection does not invent ZIP byte verification', async (t) => {
  const f = await cliFixture(t);
  const out = path.join(f.directory, 'inspection.json');
  const run = await f.invoke('pr-artifacts.mjs', [
    '--plan',
    path.join(f.directory, 'plan.json'),
    '--bindings',
    path.join(f.directory, 'bindings.json'),
    '--roles',
    '["guard"]',
    '--out',
    out,
  ]);
  const result = JSON.parse(readFileSync(out));
  assert.equal(run.exit, 0, run.stderr);
  assert.equal(result.status, 'PASS');
  assert.equal(result.matched.length, 1);
  assert.deepEqual(result.verifiedBytes, []);
  assert.equal(f.calls.filter((url) => url.endsWith('/zip')).length, 0);
  assertVerdict(run.stdout, 'pr-artifacts', 'PASS');
});

test('valid record bindings never waive missing behavioural documents', async (t) => {
  const f = await cliFixture(t);
  const out = path.join(f.directory, 'record');
  const run = await f.invoke('pr-record.mjs', [
    '--evidence',
    f.directory,
    '--bindings',
    path.join(f.directory, 'bindings.json'),
    '--out',
    out,
  ]);
  const record = JSON.parse(readFileSync(path.join(out, 'record.json')));
  assert.equal(run.exit, 0, run.stderr);
  assert.equal(record.status, 'INCOMPLETE');
  assert.equal(record.certified, false);
  assert.equal(record.artifactBinding.status, 'PASS');
  assert.deepEqual(
    record.artifactBinding.matched.map((row) => row.role).sort(),
    [...record.artifactBinding.roles].sort()
  );
  assert.ok(record.incomplete.length > 0);
  assertVerdict(run.stdout, 'pr-record', 'INCOMPLETE');
});

test('missing required inventory entries are INCOMPLETE, never producer verification', async (t) => {
  const f = await cliFixture(t, (f) => {
    f.actual = [];
  });
  const out = path.join(f.directory, 'missing.json');
  const run = await f.invoke('pr-artifacts.mjs', [
    '--plan',
    path.join(f.directory, 'plan.json'),
    '--bindings',
    path.join(f.directory, 'bindings.json'),
    '--roles',
    '["guard"]',
    '--action',
    'download',
    '--directory',
    path.join(f.directory, 'archives'),
    '--out',
    out,
  ]);
  const result = JSON.parse(readFileSync(out));
  assert.equal(run.exit, 1, run.stderr);
  assert.equal(result.status, 'INCOMPLETE');
  assert.deepEqual(result.verifiedBytes, []);
  assert.equal(f.calls.filter((url) => url.endsWith('/zip')).length, 0);
  assertVerdict(run.stdout, 'pr-artifacts', 'INCOMPLETE');
});
