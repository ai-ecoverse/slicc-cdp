import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

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
]) {
  test(`independent record CLI revalidates bios: ${mode}`, (t) => {
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
        artifact: {
          name: `${side}-canary`,
          url: `https://github.com/fixture/lane/actions/runs/42/artifacts/${side === 'base' ? 1 : 2}`,
        },
      };
    }
    const row = { shard: 'guard', test: 'guarded CDP smoke', log: 'failure' };
    if (mode === 'forged PASS with regression') {
      bios.results.candidate.counts.regression = 1;
      bios.results.candidate.regressions = [row];
    }
    if (mode === 'all forged digest claims') {
      bios.calls.candidate['candidate-sha256'] = 'f'.repeat(64);
      bios.results.candidate.overrides[0].sha256 = 'f'.repeat(64);
      bios.results.candidate.candidates[0].status =
        bios.results.candidate.candidates[0].status.replace(/[a-f0-9]{64}/, 'f'.repeat(64));
    }
    if (mode === 'missing paired result') delete bios.results.base;
    if (mode === 'both flags recomputed') {
      bios.results.candidate.counts.both = 1;
      bios.results.candidate.both = [row];
    }
    save(path.join(directory, 'plan.json'), plan);
    save(path.join(directory, 'bios.json'), bios);
    const out = path.join(directory, 'record');
    const run = spawnSync(
      process.execPath,
      [path.join(root, 'scripts/cert/pr-record.mjs'), '--evidence', directory, '--out', out],
      {
        encoding: 'utf8',
        env: {
          ...process.env,
          CERT_PR_PACKS: packs,
          CERT_PR_ASSETS: path.join(directory, 'absent-assets'),
          CERT_UPSTREAM: JSON.stringify({ bios: 'success' }),
        },
      }
    );
    assert.equal(run.status, 0, run.stderr);
    const record = read(path.join(out, 'record.json'));
    if (mode === 'complete')
      assert.ok(!record.incomplete.some((reason) => reason.startsWith('bios:')));
    if (mode === 'forged PASS with regression')
      assert.match(record.failures.join(';'), /candidate: regressions/);
    if (mode === 'all forged digest claims')
      assert.match(record.failures.join(';'), /override pack digest mismatch/);
    if (mode === 'missing paired result')
      assert.match(record.incomplete.join(';'), /base: exact pack or canary result absent/);
    if (mode === 'both flags recomputed')
      assert.match(record.flags.join(';'), /failures also present on bios pins/);
  });
}
