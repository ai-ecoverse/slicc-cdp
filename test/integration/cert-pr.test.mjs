import assert from 'node:assert/strict';
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { assertVerdict, stdoutDocument } from './cert-cli-verdict.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const repository = 'fixture/lane';
const sha256 = (file) => createHash('sha256').update(readFileSync(file)).digest('hex');
const read = (file) => JSON.parse(readFileSync(file, 'utf8'));
const save = (file, value) => {
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, JSON.stringify(value));
  return file;
};

test('test CLI children strip ambient CERT and GITHUB values unless explicitly overridden', () => {
  const ambient = {
    CERT_PR_PACKS: '/ambient/packs',
    CERT_RUNNER: 'ambient',
    CERT_REVIEW_MODEL: 'ambient',
    GITHUB_API_URL: 'https://ambient.invalid',
    GITHUB_OUTPUT: '/ambient/output',
    GITHUB_RUN_ID: 'ambient',
    KEEP_FIXTURE: 'kept',
  };
  const env = cliEnvironment({}, ambient);
  assert.deepEqual(env, { KEEP_FIXTURE: 'kept' });
  const child = spawnSync(
    process.execPath,
    [
      '-e',
      'process.stdout.write(JSON.stringify(Object.fromEntries(Object.entries(process.env).filter(([key]) => /^(CERT_|GITHUB_|KEEP_FIXTURE)/.test(key)))))',
    ],
    {
      env,
      encoding: 'utf8',
    }
  );
  assert.equal(child.status, 0, child.stderr);
  assert.deepEqual(JSON.parse(child.stdout), { KEEP_FIXTURE: 'kept' });
  assert.deepEqual(cliEnvironment({ CERT_PR_PACKS: '/case/packs', GITHUB_RUN_ID: '42' }, ambient), {
    KEEP_FIXTURE: 'kept',
    CERT_PR_PACKS: '/case/packs',
    GITHUB_RUN_ID: '42',
  });
});

function fixture(t, file = 'src/client.js', contents = 'changed\n') {
  const directory = mkdtempSync(path.join(tmpdir(), 'slicc-pr-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const git = (...args) =>
    execFileSync('git', ['-c', 'commit.gpgsign=false', ...args], {
      cwd: directory,
      encoding: 'utf8',
    }).trim();
  git('init', '--quiet', '--initial-branch=main');
  git('config', 'user.name', 'Fixture');
  git('config', 'user.email', 'fixture@example.invalid');
  save(path.join(directory, 'package.json'), {
    name: '@ai-ecoverse/slicc-cdp',
    version: '0.0.0',
    config: { chrome: '156.0.8078.4' },
    devDependencies: { '@ai-ecoverse/slicc-browsers': '1.7.3' },
  });
  writeFileSync(path.join(directory, 'rust-toolchain.toml'), '[toolchain]\nchannel = "1.99.1"\n');
  writeFileSync(path.join(directory, 'Cargo.lock'), 'version = 4\n');
  git('add', '.');
  git('commit', '--quiet', '-m', 'initial');
  const base = git('rev-parse', 'HEAD');
  git('checkout', '--quiet', '-b', 'proposal');
  mkdirSync(path.dirname(path.join(directory, file)), { recursive: true });
  writeFileSync(
    path.join(directory, file),
    typeof contents === 'function' ? contents(read(path.join(directory, file))) : contents
  );
  git('add', '.');
  git('commit', '--quiet', '-m', 'ci: public change');
  const head = git('rev-parse', 'HEAD');
  git('checkout', '--quiet', 'main');
  git('merge', '--quiet', '--no-ff', 'proposal', '-m', 'Merge pull request #10');
  const merge = git('rev-parse', 'HEAD');
  const pull = {
    number: 10,
    state: 'open',
    changed_files: 1,
    draft: false,
    head: { sha: head, repo: { full_name: repository } },
    base: { sha: base, ref: 'main', repo: { full_name: repository } },
    labels: [],
  };
  const self = { status: 'PASS', repository, members: [{ pr: 10, head, status: 'PASS' }] };
  return {
    directory,
    git,
    base,
    head,
    merge,
    pull,
    self,
    files: [{ filename: file, status: 'added' }],
  };
}

async function server(t, f, extras = {}) {
  const calls = [];
  const api = createServer((req, res) => {
    const pathname = new URL(req.url, 'http://fixture').pathname.replace(
      `/repos/${repository}/`,
      ''
    );
    let body = '';
    req.on('data', (chunk) => {
      body += chunk;
    });
    req.on('end', () => {
      calls.push({ method: req.method, pathname, body: body ? JSON.parse(body) : null });
      const send = (value, status = 200) => {
        res.writeHead(status, { 'content-type': 'application/json', connection: 'close' });
        res.end(JSON.stringify(value));
      };
      if (extras.route?.(req, pathname, res)) return;
      if (req.method !== 'GET') return send({ id: 90 });
      if (pathname === 'pulls/10') return send(f.pull);
      if (pathname === 'pulls/10/files') return send(f.files);
      if (pathname === 'issues/10/comments') return send([]);
      if (pathname === 'issues/10/timeline') return send(extras.timeline ?? []);
      return send({ error: pathname }, 404);
    });
  });
  await new Promise((resolve) => api.listen(0, '127.0.0.1', resolve));
  t.after(
    () =>
      new Promise((resolve) => {
        api.closeAllConnections();
        api.close(resolve);
      })
  );
  return { origin: `http://127.0.0.1:${api.address().port}`, calls };
}

function cliEnvironment(overrides = {}, ambient = process.env) {
  return {
    ...Object.fromEntries(Object.entries(ambient).filter(([key]) => !/^(CERT_|GITHUB_)/.test(key))),
    ...overrides,
  };
}

async function cli(script, argv, f, api, extra = {}) {
  const child = spawn(process.execPath, [path.join(root, 'scripts/cert', script), ...argv], {
    cwd: f.directory,
    env: cliEnvironment({
      GH_TOKEN: '',
      GITHUB_API_URL: api.origin,
      GITHUB_RUN_ID: '42',
      GITHUB_RUN_ATTEMPT: '1',
      ...extra,
    }),
  });
  let stdout = '';
  let stderr = '';
  child.stdout.on('data', (chunk) => {
    stdout += chunk;
  });
  child.stderr.on('data', (chunk) => {
    stderr += chunk;
  });
  const exit = await new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('close', resolve);
  });
  if (['pr-plan.mjs', 'pr-report.mjs'].includes(script)) {
    const document = stdoutDocument(stdout);
    const status =
      script === 'pr-plan.mjs'
        ? document.go || document.status === 'PASS'
          ? 'PASS'
          : document.status
        : document.status === 'NOOP'
          ? 'PASS'
          : document.status;
    const line = assertVerdict(stdout, script.replace(/\.mjs$/, ''), status);
    if (script === 'pr-plan.mjs')
      assert.equal(line, `pr-plan: ${status} go=${document.go} document=${document.status}`);
    else assert.equal(line, `pr-report: ${status} document=${document.status}`);
  }
  if (script === 'pr-record.mjs')
    assertVerdict(
      stdout,
      'pr-record',
      read(path.join(argv[argv.indexOf('--out') + 1], 'record.json')).status
    );
  return { exit, stdout, stderr };
}

async function planned(f, api, event = {}) {
  const file = save(path.join(f.directory, 'event.json'), {
    repository: { full_name: repository },
    pull_request: f.pull,
    action: 'synchronize',
    ...event,
  });
  const self = save(path.join(f.directory, 'self.json'), f.self);
  const out = path.join(f.directory, 'plan.json');
  const result = await cli(
    'pr-plan.mjs',
    ['--event', file, '--self', self, '--directory', f.directory, '--merge', f.merge, '--out', out],
    f,
    api
  );
  assert.equal(result.exit, 0, result.stderr);
  return { file, out, plan: read(out) };
}

for (const [name, file, behaviour, testOnly] of [
  ['source behaviour', 'src/client.js', true, false],
  ['unknown build configuration', 'build-custom.config', true, false],
  ['dependency lockfile', 'package-lock.json', true, false],
  ['documentation not behaviour', 'README.md', false, false],
  ['test-only full review contribution', 'test/integration/a.test.mjs', true, true],
  ['reviewer path behaviour', 'scripts/cert/reviewer.mjs', true, false],
]) {
  test(`PR cert planner: ${name}`, async (t) => {
    const f = fixture(t, file);
    const api = await server(t, f);
    const { plan } = await planned(f, api);
    assert.equal(plan.behaviour, behaviour);
    assert.equal(plan.go, behaviour);
    assert.equal(plan.testOnly, testOnly);
    assert.equal(plan.status, behaviour ? 'INCOMPLETE' : 'PASS');
    if (behaviour) {
      assert.equal(plan.commit, f.merge);
      assert.equal(plan.base, f.base);
      assert.equal(plan.head, f.head);
    } else assert.equal(plan.reason, 'not behaviour');
  });
}

test('PR cert planner runs full review for a scripts-only package postinstall change', async (t) => {
  const f = fixture(t, 'package.json', (manifest) =>
    JSON.stringify({ ...manifest, scripts: { postinstall: 'node postinstall.mjs' } })
  );
  const base = JSON.parse(f.git('show', `${f.base}:package.json`));
  const head = JSON.parse(f.git('show', `${f.head}:package.json`));
  assert.equal(head.scripts.postinstall, 'node postinstall.mjs');
  delete head.scripts;
  assert.deepEqual(head, base);
  f.files[0].status = 'modified';
  const api = await server(t, f);
  const { plan } = await planned(f, api);
  assert.equal(plan.behaviour, true);
  assert.equal(plan.go, true);
  assert.equal(plan.testOnly, false);
  assert.equal(plan.reason, 'behaviour review');
  assert.equal(plan.commit, f.merge);
});

test('PR cert planner: a draft waits, ready_for_review starts review, later pushes bind a new real head', async (t) => {
  const f = fixture(t);
  const api = await server(t, f);
  f.pull.draft = true;
  const draft = (await planned(f, api, { action: 'opened' })).plan;
  assert.equal(draft.go, false);
  assert.equal(draft.report, true);
  assert.equal(draft.status, 'INCOMPLETE');
  assert.equal(draft.reason, 'not reviewed: draft, mark it ready for review');
  delete f.pull.draft;
  assert.equal((await planned(f, api)).plan.go, false, 'a missing draft flag counts as a draft');
  f.pull.draft = false;
  assert.equal((await planned(f, api, { action: 'ready_for_review' })).plan.go, true);
  f.git('checkout', '--quiet', 'proposal');
  writeFileSync(path.join(f.directory, 'src/client.js'), 'later\n');
  f.git('add', 'src/client.js');
  f.git('commit', '--quiet', '-m', 'fix: later push');
  f.head = f.git('rev-parse', 'HEAD');
  f.pull.head.sha = f.head;
  f.self.members[0].head = f.head;
  f.git('checkout', '--quiet', 'main');
  f.git('merge', '--quiet', '--no-ff', 'proposal', '-m', 'Merge later push');
  f.merge = f.git('rev-parse', 'HEAD');
  const latest = (await planned(f, api)).plan;
  assert.equal(latest.go, true);
  assert.equal(latest.head, f.head);
  assert.equal(latest.commit, f.merge);
  f.pull.draft = true;
  const converted = (await planned(f, api, { action: 'converted_to_draft' })).plan;
  assert.equal(converted.go, false);
  assert.equal(converted.report, true);
  assert.match(converted.reason, /draft/);
});

test('PR cert planner ignores unrelated labels, rejects stale heads, incomplete files and removed readiness', async (t) => {
  const f = fixture(t);
  const api = await server(t, f);
  assert.equal(
    (await planned(f, api, { action: 'labeled', label: { name: 'other' } })).plan.report,
    false
  );
  const original = structuredClone(f.pull);
  f.pull.head.sha = f.base;
  assert.equal((await planned(f, api, { pull_request: original })).plan.report, false);
  f.pull.head.sha = f.head;
  f.pull.changed_files = 2;
  assert.match((await planned(f, api)).plan.issues.join(';'), /listing incomplete/);
  f.pull.changed_files = 1;
  for (const event of [
    { action: 'labeled', label: { name: 'merge-ok' } },
    { action: 'unlabeled', label: { name: 'merge-ok' } },
    { action: 'unlabeled', label: { name: 'other' } },
  ]) {
    const quiet = (await planned(f, api, event)).plan;
    assert.equal(quiet.report, false, JSON.stringify(event));
    assert.equal(quiet.go, false);
    assert.equal(quiet.reason, 'unrelated label');
  }
});

test('PR cert planner honours shared reviewer refusal and binds the receipt to the exact head', async (t) => {
  const f = fixture(t, 'scripts/cert/reviewer.mjs');
  const api = await server(t, f);
  f.self.status = 'FAIL';
  f.self.members[0].status = 'FAIL';
  assert.equal((await planned(f, api)).plan.go, false);
  assert.match((await planned(f, api)).plan.reason, /needs coordinator review/);
  f.self.status = 'PASS';
  f.self.members[0].status = 'PASS';
  f.self.members[0].head = f.base;
  assert.equal((await planned(f, api)).plan.go, false);
  f.self.members[0].head = f.head;
  assert.equal(
    (await planned(f, api, { action: 'labeled', label: { name: 'cert-review-approved' } })).plan.go,
    true
  );
});

test('PR cert planner treats both rename paths conservatively and refuses fork behaviour', async (t) => {
  const f = fixture(t, 'README.md');
  const api = await server(t, f);
  f.files[0].previous_filename = 'src/old.js';
  f.files[0].status = 'renamed';
  assert.equal((await planned(f, api)).plan.go, true);
  f.pull.head.repo.full_name = 'fork/lane';
  const result = (await planned(f, api)).plan;
  assert.equal(result.go, false);
  assert.match(result.reason, /fork behaviour/);
});

function certificate(plan) {
  const {
    version,
    mode,
    commit,
    baseline,
    harness,
    rust,
    vectors,
    run,
    attempt,
    repository,
    pr,
    head,
    base,
    tools,
  } = plan;
  return {
    version,
    mode,
    commit,
    baseline,
    harness,
    rust,
    vectors,
    run,
    attempt,
    repository,
    pr,
    head,
    base,
    tools,
    legacyChecksum: false,
    schema: 1,
    status: 'PASS',
    certified: true,
    behaviour: true,
    fullReviewTriggered: true,
    sha256: 'a'.repeat(64),
    required: ['rebuild', 'guard', 'integration', 'bios', 'assets-linux', 'assets-macos', 'review'],
    failures: [],
    incomplete: [],
    self: plan.self,
    line: 'fixture certified PR',
  };
}

for (const scenario of ['absent', 'malformed', 'wrong identity'])
  test(`PR cert reporter invalidates prior success for ${scenario} record on the current head`, async (t) => {
    const f = fixture(t);
    const api = await server(t, f);
    const { file, out, plan } = await planned(f, api);
    const record = path.join(f.directory, 'record.json');
    if (scenario === 'malformed') writeFileSync(record, '{');
    else if (scenario === 'wrong identity') save(record, { ...certificate(plan), head: f.base });
    const result = await cli(
      'pr-report.mjs',
      ['--event', file, '--plan', out, '--record', record],
      f,
      api
    );
    assert.equal(result.exit, 1);
    const output = stdoutDocument(result.stdout);
    assert.equal(output.status, 'INCOMPLETE');
    assert.equal(output.head, f.head);
    const statuses = api.calls.filter((call) => call.pathname === `statuses/${f.head}`);
    assert.equal(statuses.length, 1);
    assert.equal(statuses[0].body.state, 'error');
    assert.ok(!api.calls.some((call) => call.body?.state === 'success'));
  });

test('PR cert reporter: an unrelated label run posts nothing and its job is green only when the head is certified', async (t) => {
  const f = fixture(t);
  let state;
  const api = await server(t, f, {
    route(req, pathname, res) {
      if (pathname !== `commits/${f.head}/statuses`) return false;
      res.writeHead(200, { 'content-type': 'application/json', connection: 'close' });
      res.end(JSON.stringify(state ? [{ context: 'cert-review-pr', state }] : []));
      return true;
    },
  });
  const { file, out } = await planned(f, api, { action: 'labeled', label: { name: 'bug' } });
  for (const [current, exit] of [
    ['success', 0],
    ['pending', 1],
    ['failure', 1],
    [undefined, 1],
  ]) {
    state = current;
    const r = await cli('pr-report.mjs', ['--event', file, '--plan', out], f, api);
    assert.equal(r.exit, exit, `${current}: ${r.stderr}`);
    assert.equal(stdoutDocument(r.stdout).status, exit ? 'INCOMPLETE' : 'NOOP');
  }
  assert.equal(api.calls.filter((call) => call.method === 'POST').length, 0);
});

test('PR cert reporter never posts on a stale head and a PR converted to draft mid-run is not proof', async (t) => {
  const f = fixture(t);
  const api = await server(t, f);
  const { file, out, plan } = await planned(f, api);
  const record = save(path.join(f.directory, 'record.json'), certificate(plan));
  f.pull.head.sha = f.base;
  const stale = await cli(
    'pr-report.mjs',
    ['--event', file, '--plan', out, '--record', record],
    f,
    api
  );
  assert.equal(stdoutDocument(stale.stdout).status, 'NOOP');
  assert.equal(api.calls.filter((call) => call.method === 'POST').length, 0);
  f.pull.head.sha = f.head;
  const ready = await cli(
    'pr-report.mjs',
    ['--event', file, '--plan', out, '--record', record],
    f,
    api
  );
  assert.equal(ready.exit, 0, ready.stderr);
  assert.equal(stdoutDocument(ready.stdout).status, 'PASS');
  api.calls.length = 0;
  f.pull.draft = true;
  const removed = await cli(
    'pr-report.mjs',
    ['--event', file, '--plan', out, '--record', record],
    f,
    api
  );
  assert.equal(removed.exit, 1);
  assert.equal(stdoutDocument(removed.stdout).status, 'INCOMPLETE');
  assert.deepEqual(stdoutDocument(removed.stdout).incomplete, ['converted to draft: not proof']);
  assert.ok(!api.calls.some((call) => call.body?.state === 'success'));
  assert.equal(
    api.calls.find((call) => call.pathname === `statuses/${f.head}`).body.state,
    'error'
  );
});

test('PR cert reporter explicitly succeeds not behaviour without a full review', async (t) => {
  const f = fixture(t, 'README.md');
  const api = await server(t, f);
  const { file, out } = await planned(f, api);
  const result = await cli('pr-report.mjs', ['--event', file, '--plan', out], f, api);
  assert.equal(result.exit, 0, result.stderr);
  const record = stdoutDocument(result.stdout);
  assert.match(record.line, /not behaviour/);
  assert.equal(record.fullReviewTriggered, false);
  assert.equal(
    api.calls.find((call) => call.pathname === `statuses/${f.head}`).body.state,
    'success'
  );
});

function pack(f, plan, side, packs) {
  const subject =
    side === 'candidate' ? { version: plan.version, commit: plan.commit } : plan.baseline;
  const directory = path.join(packs, side);
  const content = path.join(f.directory, `content-${side}`);
  save(path.join(content, 'package/package.json'), {
    name: '@ai-ecoverse/slicc-cdp',
    version: subject.version,
    gitHead: subject.commit,
  });
  mkdirSync(path.join(content, 'package/bin'), { recursive: true });
  for (const command of ['playwright-cli', 'curlwright'])
    writeFileSync(path.join(content, 'package/bin', `${command}.wasm`), 'fixture wasm');
  mkdirSync(directory, { recursive: true });
  execFileSync('tar', ['-czf', path.join(directory, 'package.tgz'), '-C', content, 'package']);
  const files = [
    { file: 'package.tgz', sha256: sha256(path.join(directory, 'package.tgz')), package: true },
  ];
  for (const command of ['playwright-cli', 'curlwright']) {
    const file = `${command}-host`;
    writeFileSync(path.join(directory, file), 'fixture native');
    files.push({ file, sha256: sha256(path.join(directory, file)) });
  }
  save(path.join(directory, 'build.json'), {
    ...certificate(plan),
    side,
    subject,
    target: 'wasm32-wasip1',
    files,
  });
}

test('PR cert code-only record names missing bios evidence and gives pack integrity defects FAIL precedence', async (t) => {
  const f = fixture(t);
  const api = await server(t, f);
  const { plan } = await planned(f, api);
  plan.proofs = { tests: [], known: [], fixes: [] };
  const evidence = path.join(f.directory, 'evidence');
  save(path.join(evidence, 'plan.json'), plan);
  const packs = path.join(f.directory, 'packs');
  pack(f, plan, 'candidate', packs);
  pack(f, plan, 'base', packs);
  const out = path.join(f.directory, 'result');
  const options = ['--evidence', evidence, '--out', out];
  const result = await cli('pr-record.mjs', options, f, api, { CERT_PR_PACKS: packs });
  assert.equal(result.exit, 0, result.stderr);
  assert.equal(read(path.join(out, 'record.json')).status, 'INCOMPLETE');
  assert.match(read(path.join(out, 'record.json')).incomplete.join(';'), /bios/);
  save(path.join(evidence, 'guard.json'), {
    ...certificate(plan),
    which: 'guard',
    sha256: sha256(path.join(packs, 'candidate/package.tgz')),
    executions: {
      positive: { files: ['guard.test.mjs'], rows: 'malformed', attempts: [{ exit: 0 }] },
    },
  });
  const malformed = await cli('pr-record.mjs', options, f, api, { CERT_PR_PACKS: packs });
  assert.equal(malformed.exit, 0, malformed.stderr);
  assert.equal(read(path.join(out, 'record.json')).status, 'INCOMPLETE');
  assert.match(read(path.join(out, 'record.json')).incomplete.join(';'), /execution absent/);
  writeFileSync(path.join(packs, 'candidate/playwright-cli-host'), 'tampered');
  const corrupt = await cli('pr-record.mjs', options, f, api, { CERT_PR_PACKS: packs });
  assert.equal(corrupt.exit, 0, corrupt.stderr);
  assert.equal(read(path.join(out, 'record.json')).status, 'FAIL');
  assert.match(read(path.join(out, 'record.json')).failures.join(';'), /digest mismatch/);
});

test('PR cert reporter deduplicates test-only full-review count across reruns', async (t) => {
  const f = fixture(t, 'test/integration/a.test.mjs');
  let sticky = [];
  const api = await server(t, f, {
    route(req, pathname, res) {
      if (pathname === 'issues/10/comments' && req.method === 'GET') {
        res.writeHead(200, { 'content-type': 'application/json', connection: 'close' });
        res.end(JSON.stringify(sticky));
        return true;
      }
      if (
        (pathname === 'issues/10/comments' && req.method === 'POST') ||
        (pathname === 'issues/comments/90' && req.method === 'PATCH')
      )
        sticky = [
          { id: 90, user: { login: 'github-actions[bot]' }, body: api.calls.at(-1).body.body },
        ];
      return false;
    },
  });
  const { file, out, plan } = await planned(f, api);
  const record = save(path.join(f.directory, 'record.json'), certificate(plan));
  for (let attempt = 0; attempt < 2; attempt++) {
    const result = await cli(
      'pr-report.mjs',
      ['--event', file, '--plan', out, '--record', record],
      f,
      api
    );
    assert.equal(result.exit, 0, result.stderr);
    assert.equal(stdoutDocument(result.stdout).testOnlyPRCountContribution, 1);
  }
  assert.equal(
    api.calls.filter((call) => call.method === 'POST' && call.pathname === 'issues/10/comments')
      .length,
    1
  );
  assert.equal(
    api.calls.filter((call) => call.method === 'PATCH' && call.pathname === 'issues/comments/90')
      .length,
    1
  );
});

test('PR bios public input planner binds both local packs and labels without pinning an unmerged workflow', async (t) => {
  const f = fixture(t);
  const api = await server(t, f);
  const { out, plan } = await planned(f, api);
  const packs = path.join(f.directory, 'packs');
  for (const side of ['base', 'candidate']) pack(f, plan, side, packs);
  const directory = path.join(f.directory, 'unpacked-inputs');
  const directoryArgs = /\bdirectory:\s*\{/.test(
    readFileSync(path.join(root, 'scripts/cert/pr-bios-inputs.mjs'), 'utf8')
  )
    ? ['--directory', directory]
    : [];
  const output = path.join(f.directory, 'inputs.json');
  const call = () =>
    cli('pr-bios-inputs.mjs', ['--plan', out, '--out', output, ...directoryArgs], f, api, {
      CERT_PR_PACKS: packs,
    });
  const result = await call();
  assert.equal(result.exit, 0, result.stderr);
  const inputs = read(output);
  const expectedPin = read(path.join(root, 'scripts/cert/pr-bios.json'));
  assert.equal(inputs.status, 'PASS');
  assert.equal(inputs.ready, true);
  for (const side of ['base', 'candidate']) {
    const row = inputs.calls[side];
    assert.equal(row.label, side);
    assert.equal(row['bios-ref'], expectedPin.commit);
    assert.equal(row.repeat, 3);
    assert.equal(row.runner, 'ubuntu-latest');
    assert.equal(row['candidate-package'], '@ai-ecoverse/slicc-cdp');
    assert.equal(row['candidate-repo'], repository);
    assert.equal(row['candidate-run'], plan.run);
    assert.equal(row['candidate-sha256'], sha256(path.join(packs, side, 'package.tgz')));
    assert.deepEqual(JSON.parse(row.shards), expectedPin.shards);
  }
  assert.notEqual(
    inputs.calls.base['candidate-artifact'],
    inputs.calls.candidate['candidate-artifact']
  );
  writeFileSync(path.join(packs, 'candidate/package.tgz'), 'tampered');
  assert.equal((await call()).exit, 1);
  assert.equal(read(output).status, 'FAIL');
  assert.equal(read(output).ready, false);
});

test('PR bios slot stays INCOMPLETE without paired byte-bound packs and canary results', async (t) => {
  const f = fixture(t);
  const api = await server(t, f);
  const { out } = await planned(f, api);
  const evidence = path.join(f.directory, 'bios.json');
  const result = await cli('pr-bios.mjs', ['--plan', out, '--out', evidence], f, api);
  assert.equal(result.exit, 0, result.stderr);
  const bios = read(evidence);
  assert.equal(bios.status, 'INCOMPLETE');
  assert.equal(bios.pin.workflow, '.github/workflows/canary.yml');
  const expectedPin = read(path.join(root, 'scripts/cert/pr-bios.json'));
  assert.equal(bios.pin.commit, expectedPin.commit);
  assert.deepEqual(bios.pin.shards, expectedPin.shards);
  assert.deepEqual(
    bios.subjects.map((item) => item.side),
    ['base', 'candidate']
  );
  assert.match(bios.reason, /exact local PR packs unavailable/);
});

for (const scenario of [
  { name: 'accepts exact-head immutable record from the trusted PR run' },
  { name: 'refuses stale run head', stale: true },
  { name: 'refuses a corrupt artifact digest', corrupt: true },
  { name: 'refuses a non-bot status', actor: 'trieloff' },
  { name: 'refuses expired record artifacts', expired: true },
  { name: 'refuses absent fresh group guard', missingSelf: true },
  { name: 'refuses an unexplained one-parent group', linear: true },
]) {
  test(`PR cert group ${scenario.name}`, async (t) => {
    const f = fixture(t);
    const archive = path.join(f.directory, 'record.zip');
    const route = (req, pathname, res) => {
      const send = (value) => {
        res.writeHead(200, { 'content-type': 'application/json', connection: 'close' });
        res.end(JSON.stringify(value));
        return true;
      };
      if (pathname === `commits/${f.head}/pulls`) return send([f.pull]);
      if (pathname === `commits/${f.head}/statuses`)
        return send([
          {
            context: 'cert-review-pr',
            state: 'success',
            creator: { login: scenario.actor ?? 'github-actions[bot]' },
            target_url: `https://github.com/${repository}/actions/runs/42`,
          },
        ]);
      if (pathname === 'actions/runs/42')
        return send({
          id: 42,
          run_attempt: 1,
          path: '.github/workflows/cert-review-pr.yml',
          event: 'pull_request',
          repository: { full_name: repository },
          head_sha: scenario.stale ? f.base : f.head,
          status: 'completed',
          conclusion: 'success',
        });
      if (pathname === 'actions/runs/42/artifacts')
        return send({
          total_count: 1,
          artifacts: [
            {
              id: 90,
              name: `cert-pr-record-10-${f.head}-1`,
              digest: `sha256:${scenario.corrupt ? 'a'.repeat(64) : sha256(archive)}`,
              expired: !!scenario.expired,
            },
          ],
        });
      if (pathname === 'actions/artifacts/90/zip') {
        res.writeHead(200, { connection: 'close' });
        res.end(readFileSync(archive));
        return true;
      }
      return false;
    };
    const api = await server(t, f, { route });
    const { plan } = await planned(f, api);
    const record = certificate(plan);
    save(path.join(f.directory, 'record.json'), record);
    execFileSync('zip', ['-j', archive, path.join(f.directory, 'record.json')]);
    const head = scenario.linear ? f.head : f.merge;
    const event = save(path.join(f.directory, 'group.json'), {
      repository: { full_name: repository },
      merge_group: { base_sha: f.base, head_sha: head },
    });
    const self = save(path.join(f.directory, 'group-self.json'), {
      status: 'PASS',
      repository,
      base: f.base,
      head,
      members: [{ pr: 10, head: f.head, status: 'PASS' }],
    });
    const options = ['--action', 'group', '--event', event];
    if (!scenario.missingSelf) options.push('--self', self);
    const result = await cli('pr-report.mjs', options, f, api);
    const refused = Object.keys(scenario).length > 1;
    assert.equal(result.exit, refused ? 1 : 0, result.stderr);
    const group = stdoutDocument(result.stdout);
    assert.equal(group.status, scenario.corrupt ? 'FAIL' : refused ? 'INCOMPLETE' : 'PASS');
    const status = api.calls.find((call) => call.pathname === `statuses/${head}`);
    assert.equal(status.body.state, scenario.corrupt ? 'failure' : refused ? 'error' : 'success');
    if (!refused) {
      assert.equal(group.records[0].head, f.head);
      assert.equal(group.records[0].artifact, 90);
    }
    t.after(() => rmSync(path.join(root, '.cert/group/42'), { recursive: true, force: true }));
  });
}
