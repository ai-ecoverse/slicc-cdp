import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';

const root = path.resolve(import.meta.dirname, '../..');
const repository = 'fixture/stdout-order';

function fixture(t) {
  const directory = mkdtempSync(path.join(tmpdir(), 'cert-stdout-order-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const git = (...args) =>
    execFileSync('git', ['-c', 'commit.gpgsign=false', ...args], {
      cwd: directory,
      encoding: 'utf8',
    }).trim();
  git('init', '-q', '--initial-branch=main');
  git('config', 'user.name', 'Stdout fixture');
  git('config', 'user.email', 'stdout@example.invalid');
  writeFileSync(
    path.join(directory, 'package.json'),
    JSON.stringify({ name: 'stdout-fixture', version: '0.0.0', config: { chrome: '156.0.8078.4' } })
  );
  writeFileSync(path.join(directory, 'rust-toolchain.toml'), '[toolchain]\nchannel = "1.99.1"\n');
  writeFileSync(path.join(directory, 'Cargo.lock'), 'version = 4\n');
  git('add', '-A', '--', ':/');
  git('commit', '-qm', 'base fixture');
  const base = git('rev-parse', 'HEAD');
  git('checkout', '-qb', 'proposal');
  mkdirSync(path.join(directory, 'src'));
  writeFileSync(path.join(directory, 'src/snapshot.js'), 'export const snapshot = true;\n');
  git('add', '-A', '--', ':/');
  git('commit', '-qm', 'snapshot fixture');
  const head = git('rev-parse', 'HEAD');
  git('checkout', '-q', 'main');
  git('merge', '-q', '--no-ff', 'proposal', '-m', 'Merge fixture');
  return { directory, base, head, merge: git('rev-parse', 'HEAD') };
}

async function gate(command, env = {}, name = 'pr-plan') {
  const childEnv = { ...process.env, GH_TOKEN: '', ...env };
  delete childEnv.NODE_TEST_CONTEXT;
  const child = spawn(
    process.execPath,
    [
      path.join(root, 'scripts/gate.mjs'),
      name,
      '--accept',
      'PASS,FAIL,INCOMPLETE',
      '--',
      process.execPath,
      ...command,
    ],
    {
      cwd: root,
      env: childEnv,
      stdio: ['ignore', 'pipe', 'pipe'],
    }
  );
  const output = { stdout: '', stderr: '' };
  child.stdout.on('data', (chunk) => {
    output.stdout += chunk;
  });
  child.stderr.on('data', (chunk) => {
    output.stderr += chunk;
  });
  const exit = await new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('close', resolve);
  });
  return { ...output, exit };
}

for (const size of [72, 234]) {
  test(`actual gate accepts a ${size} KiB PR plan only after complete JSON and a standalone verdict`, async (t) => {
    const f = fixture(t);
    const pull = {
      number: 7,
      state: 'open',
      changed_files: 1,
      draft: false,
      head: { sha: f.head, repo: { full_name: repository } },
      base: { sha: f.base, ref: 'main', repo: { full_name: repository } },
    };
    const files = [
      { filename: 'src/snapshot.js', status: 'added', patch: '+' + 's'.repeat(size * 1024) },
    ];
    const api = createServer((request, response) => {
      const route = new URL(request.url, 'http://fixture').pathname;
      const value = route.endsWith('/files') ? files : pull;
      response.writeHead(200, { 'content-type': 'application/json', connection: 'close' });
      response.end(JSON.stringify(value));
    });
    await new Promise((resolve) => api.listen(0, '127.0.0.1', resolve));
    t.after(
      () =>
        new Promise((resolve) => {
          api.closeAllConnections();
          api.close(resolve);
        })
    );
    const event = path.join(f.directory, 'event.json');
    const self = path.join(f.directory, 'self.json');
    const out = path.join(f.directory, 'plan.json');
    writeFileSync(
      event,
      JSON.stringify({
        action: 'synchronize',
        repository: { full_name: repository },
        pull_request: pull,
      })
    );
    writeFileSync(
      self,
      JSON.stringify({
        status: 'PASS',
        repository,
        members: [{ pr: 7, head: f.head, status: 'PASS' }],
      })
    );
    const result = await gate(
      [
        path.join(root, 'scripts/cert/pr-plan.mjs'),
        '--event',
        event,
        '--self',
        self,
        '--directory',
        f.directory,
        '--merge',
        f.merge,
        '--out',
        out,
      ],
      {
        GITHUB_API_URL: `http://127.0.0.1:${api.address().port}`,
        GITHUB_RUN_ID: '42',
        GITHUB_RUN_ATTEMPT: '1',
      }
    );
    assert.equal(result.exit, 0, result.stderr);
    const lines = result.stdout.trimEnd().split('\n');
    assert.equal(lines.length, 2);
    assert.ok(Buffer.byteLength(lines[0]) > size * 1024);
    const document = JSON.parse(lines[0]);
    assert.deepEqual(document, JSON.parse(readFileSync(out)));
    assert.equal(document.head, f.head);
    assert.equal(document.go, true);
    assert.equal(document.status, 'INCOMPLETE');
    assert.equal(lines[1], 'pr-plan: PASS go=true document=INCOMPLETE');
  });
}

for (const [name, output] of [
  ['embedded', JSON.stringify({ patch: 'pr-plan: PASS go=true document=INCOMPLETE' }) + '\n'],
  ['partial', 'pr-plan: PA'],
]) {
  test(`actual gate still refuses a ${name} verdict even when the child exits zero`, async () => {
    const result = await gate(['-e', `process.stdout.write(${JSON.stringify(output)})`]);
    assert.equal(result.exit, 1);
    assert.match(result.stderr, /FAIL no "pr-plan: PASS\|FAIL\|INCOMPLETE" verdict line/);
  });
}
