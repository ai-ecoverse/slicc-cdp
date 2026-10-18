import assert from 'node:assert/strict';
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { artifactNames } from '../../scripts/cert/pr-artifact-binding.mjs';
import { prIdentity } from '../../scripts/cert/pr-common.mjs';
import { assertVerdict } from './cert-cli-verdict.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const save = (file, value) => writeFileSync(file, JSON.stringify(value));
const read = (file) => JSON.parse(readFileSync(file));
const digest = (file) => createHash('sha256').update(readFileSync(file)).digest('hex');

for (const mode of [
  'complete',
  'forged PASS with regression',
  'all forged digest claims',
  'missing paired result',
  'both flags recomputed',
  'strict complete outputs',
  'strict forged PASS callee digest',
  'strict split result representations',
  'both producer rejections are pre-existing flags',
  'base producer rejection is improved by candidate',
]) {
  test(`independent record CLI revalidates bios: ${mode}`, async (t) => {
    const directory = mkdtempSync(path.join(tmpdir(), 'slicc-bios-record-'));
    t.after(() => rmSync(directory, { recursive: true, force: true }));
    const plan = {
      mode: 'pr',
      go: true,
      pr: 273,
      head: 'a'.repeat(40),
      commit: 'b'.repeat(40),
      base: 'c'.repeat(40),
      tools: 'c'.repeat(40),
      harness: 'c'.repeat(40),
      rust: '1.99.1',
      vectors: '1.7.6',
      version: '0.0.0-pr.273.42',
      baseline: { version: '0.0.0-base.273.42', commit: 'c'.repeat(40) },
      run: '42',
      attempt: '1',
      repository: 'fixture/lane',
      proofs: [],
      issues: [],
    };
    const packs = path.join(directory, 'packs');
    const bios = {
      ...plan,
      which: 'bios',
      status: 'PASS',
      pin: JSON.parse(readFileSync(path.join(root, 'scripts/cert/pr-bios.json'))),
      calls: {},
      results: {},
      flags: [],
    };
    for (const side of ['base', 'candidate']) {
      const subject =
        side === 'base' ? plan.baseline : { version: plan.version, commit: plan.commit };
      const contents = path.join(directory, side);
      const packed = path.join(packs, side);
      mkdirSync(path.join(contents, 'package/bin'), { recursive: true });
      mkdirSync(packed, { recursive: true });
      save(path.join(contents, 'package/package.json'), {
        name: '@ai-ecoverse/slicc-cdp',
        version: subject.version,
        gitHead: subject.commit,
      });
      for (const cmd of ['playwright-cli', 'curlwright'])
        writeFileSync(path.join(contents, `package/bin/${cmd}.wasm`), side);
      execFileSync('tar', ['-czf', 'package.tgz', '-C', contents, 'package'], { cwd: packed });
      const sha256 = digest(path.join(packed, 'package.tgz'));
      const files = [{ file: 'package.tgz', package: true, sha256 }];
      for (const cmd of ['playwright-cli', 'curlwright']) {
        writeFileSync(path.join(packed, `${cmd}-host`), 'never executable');
        files.push({ file: `${cmd}-host`, sha256: digest(path.join(packed, `${cmd}-host`)) });
      }
      save(path.join(packed, 'build.json'), {
        ...plan,
        side,
        subject,
        target: 'wasm32-wasip1',
        files,
      });
      const artifact = `cert-pr-pack-273-${plan.head}-1-${side}`;
      bios.calls[side] = {
        label: side,
        overrides: '[]',
        runner: 'ubuntu-latest',
        shards: JSON.stringify(bios.pin.shards),
        'bios-ref': bios.pin.commit,
        'candidate-package': '@ai-ecoverse/slicc-cdp',
        'candidate-artifact': artifact,
        'candidate-run': '42',
        'candidate-sha256': sha256,
        'candidate-repo': 'fixture/lane',
        repeat: 3,
      };
      bios.results[side] = {
        ok: true,
        run: 'canary',
        biosRef: bios.pin.commit,
        repeat: 3,
        url: 'https://github.com/fixture/lane/actions/runs/42',
        overrides: [{ name: '@ai-ecoverse/slicc-cdp', version: subject.version, sha256 }],
        candidates: [
          {
            package: `@ai-ecoverse/slicc-cdp@${subject.version}`,
            via: 'artifact',
            status: `candidate (artifact 42/${artifact}, sha256 ${sha256})`,
          },
        ],
        counts: { pass: 20, skip: 0, fail: 0, regression: 0, both: 0, flaky: 0 },
        regressions: [],
        both: [],
        flaky: [],
        failures: [],
        missingShards: [],
        unexplainedExits: [],
        refused: [],
        ambiguous: [],
        tests: Array.from({ length: 20 }, (_, index) => ({
          key: `guard/test-${index}`,
          shard: index === 19 ? 'packages-update-cdp' : 'guard',
          test: index === 0 ? 'guarded CDP smoke' : `passing smoke ${index}`,
          verdict: 'pass',
          failed: { candidate: '0/3', baseline: '0/0' },
        })),
        artifact: {
          name: `${side}-canary`,
          url: `https://github.com/fixture/lane/actions/runs/42/artifacts/${side === 'base' ? 1 : 2}`,
        },
      };
    }
    const row = {
      shard: 'guard',
      test: 'guarded CDP smoke',
      log: 'failure',
      failed: { candidate: '3/3', baseline: '0/3' },
    };
    const markFailed = (side, kind) => {
      const result = bios.results[side];
      result.counts.pass = 19;
      result.counts[kind] = 1;
      result[kind === 'both' ? 'both' : 'regressions'] = [row];
      result.tests[0] = { ...result.tests[0], verdict: kind, failed: row.failed };
    };
    if (mode === 'forged PASS with regression') markFailed('candidate', 'regression');
    if (mode === 'all forged digest claims') {
      bios.calls.candidate['candidate-sha256'] = 'f'.repeat(64);
      bios.results.candidate.overrides[0].sha256 = 'f'.repeat(64);
      bios.results.candidate.candidates[0].status =
        bios.results.candidate.candidates[0].status.replace(/[a-f0-9]{64}/, 'f'.repeat(64));
    }
    if (mode === 'missing paired result') delete bios.results.base;
    if (mode === 'both flags recomputed') {
      markFailed('candidate', 'both');
      markFailed('base', 'both');
    }
    if (mode === 'both producer rejections are pre-existing flags') {
      for (const side of ['base', 'candidate']) {
        markFailed(side, 'regression');
        bios.results[side].ok = false;
      }
    }
    if (mode === 'base producer rejection is improved by candidate') {
      markFailed('base', 'regression');
      bios.results.base.ok = false;
    }
    const bound = mode.startsWith('strict');
    let origin;
    if (bound) {
      const bindings = {
        schema: 1,
        ...prIdentity(plan),
        artifacts: Object.entries(artifactNames(plan)).map(([role, name], index) => ({
          role,
          name,
          id: String(index + 1),
          digest: 'e'.repeat(64),
        })),
      };
      const actual = bindings.artifacts.map((row) => ({
        id: Number(row.id),
        name: row.name,
        digest: `sha256:${row.digest}`,
        expired: false,
        workflow_run: { id: 42, head_sha: plan.head },
      }));
      const server = createServer((request, response) => {
        assert.match(request.url, /^\/repos\/fixture\/lane\/actions\/runs\/42\/artifacts\?/);
        response.setHeader('Content-Type', 'application/json');
        response.end(JSON.stringify({ total_count: actual.length, artifacts: actual }));
      });
      await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
      t.after(() => new Promise((resolve) => server.close(resolve)));
      origin = `http://127.0.0.1:${server.address().port}`;
      bios.pin = {
        ...bios.pin,
        commit: 'c2cec127b935419049568f584093d7ef03ac0721',
        bindingSchema: 1,
      };
      bios.outputs = {};
      for (const side of ['base', 'candidate']) {
        const rows = bindings.artifacts.filter((row) => row.role.startsWith(`bios-${side}-`));
        const report = rows.find((row) => row.role.endsWith('-report'));
        const pack = bindings.artifacts.find((row) => row.role === `pack-${side}`);
        Object.assign(bios.calls[side], {
          'bios-ref': bios.pin.commit,
          'candidate-binding': 'strict',
          'candidate-artifact-id': pack.id,
          'candidate-artifact-digest': pack.digest,
        });
        bios.results[side].biosRef = bios.pin.commit;
        bios.results[side].artifact.url =
          `https://github.com/fixture/lane/actions/runs/42/artifacts/${report.id}`;
        bios.outputs[side] = {
          result: structuredClone(bios.results[side]),
          'artifact-id': report.id,
          'artifact-digest': report.digest,
          artifacts: rows.map((row) => ({
            ...row,
            role: row.role.slice(`bios-${side}-`.length).split('-')[0],
            attempt: '1',
          })),
        };
      }
      if (mode === 'strict forged PASS callee digest')
        bios.outputs.candidate.artifacts[0].digest = 'f'.repeat(64);
      if (mode === 'strict split result representations') bios.results.candidate.repeat = 1;
      save(path.join(directory, 'bindings.json'), bindings);
    }
    save(path.join(directory, 'plan.json'), plan);
    save(path.join(directory, 'bios.json'), bios);
    const out = path.join(directory, 'record');
    const argv = [
      path.join(process.env.SLICC_BIOS_RECORD_SOURCE ?? root, 'scripts/cert/pr-record.mjs'),
      '--evidence',
      directory,
      '--out',
      out,
      ...(bound ? ['--bindings', path.join(directory, 'bindings.json')] : []),
    ];
    const env = {
      ...process.env,
      CERT_PR_PACKS: packs,
      CERT_PR_ASSETS: path.join(directory, 'absent-assets'),
      CERT_UPSTREAM: JSON.stringify({ bios: 'success' }),
      ...(bound ? { GITHUB_API_URL: origin, GH_TOKEN: '' } : {}),
    };
    const run = bound
      ? await new Promise((resolve, reject) => {
          const child = spawn(process.execPath, argv, { env });
          let stdout = '';
          let stderr = '';
          child.stdout.on('data', (data) => {
            stdout += data;
          });
          child.stderr.on('data', (data) => {
            stderr += data;
          });
          child.on('error', reject);
          child.on('close', (status) => resolve({ status, stdout, stderr }));
        })
      : spawnSync(process.execPath, argv, { encoding: 'utf8', env });
    assert.equal(run.status, 0, run.stderr);
    const record = read(path.join(out, 'record.json'));
    assertVerdict(run.stdout, 'pr-record', record.status);
    if (mode === 'complete')
      assert.ok(!record.incomplete.some((reason) => reason.startsWith('bios:')));
    if (mode === 'forged PASS with regression')
      assert.match(record.failures.join(';'), /candidate introduced failure/);
    if (mode === 'all forged digest claims')
      assert.match(record.failures.join(';'), /override pack digest mismatch/);
    if (mode === 'missing paired result')
      assert.match(record.incomplete.join(';'), /base: exact pack or canary result absent/);
    if (mode === 'strict complete outputs') {
      assert.equal(record.artifactBinding.status, 'PASS');
      assert.equal(record.bios.commit, 'c2cec127b935419049568f584093d7ef03ac0721');
      assert.ok(!record.incomplete.some((reason) => reason.startsWith('bios:')));
    }
    if (mode === 'strict forged PASS callee digest') {
      assert.equal(
        record.status,
        'FAIL',
        'a forged producer PASS cannot hide callee digest substitution'
      );
      assert.match(
        record.failures.join(';'),
        /callee output differs from trusted producer binding/
      );
    }
    if (mode === 'strict split result representations') {
      assert.equal(record.status, 'FAIL');
      assert.match(record.failures.join(';'), /parsed result differs from callee result output/);
    }
    if (mode === 'both producer rejections are pre-existing flags') {
      assert.ok(!record.failures.some((reason) => reason.startsWith('bios:')));
      assert.ok(!record.incomplete.some((reason) => reason.startsWith('bios:')));
      assert.match(record.flags.join(';'), /producer ok=false explained/);
      assert.match(record.flags.join(';'), /pre-existing failure on both PR packs/);
    }
    if (mode === 'base producer rejection is improved by candidate') {
      assert.ok(!record.failures.some((reason) => reason.startsWith('bios:')));
      assert.ok(!record.incomplete.some((reason) => reason.startsWith('bios:')));
      assert.match(record.flags.join(';'), /improved by candidate/);
      assert.match(record.flags.join(';'), /producer ok=false explained/);
    }

    if (mode === 'both flags recomputed')
      assert.match(record.flags.join(';'), /failures also present on bios pins/);
  });
}
