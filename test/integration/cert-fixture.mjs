import { readFileSync } from 'node:fs';
import { summary } from '@ai-ecoverse/slicc-browsers/guard-runner';

export function fixture() {
  const set = JSON.parse(
    readFileSync(
      new URL(import.meta.resolve('@ai-ecoverse/slicc-browsers/guard-vectors.json')),
      'utf8'
    )
  );
  const hash = 'a'.repeat(64);
  const commit = 'b'.repeat(40);
  const previous = 'c'.repeat(40);
  const fix = 'd'.repeat(40);
  const testFile = 'test/integration/behaviour.test.mjs';
  const plan = {
    version: '1.26.2',
    commit,
    baseline: { version: '1.25.1', commit: previous, sha256: hash },
    harness: 'e'.repeat(40),
    rust: '1.99.1',
    vectors: set.setVersion,
    run: '123',
    attempt: '1',
    fixes: [{ commit: fix, tests: [{ file: testFile, title: 'fixed behaviour' }] }],
    upstream: { rebuild: 'success', guard: 'success', integration: 'success', assets: 'success' },
  };
  const identity = { ...plan };
  delete identity.fixes;
  delete identity.upstream;
  const document = (which, data) => ({ ...identity, which, status: 'PASS', ...data });
  const report = (sha) => {
    const rows = set.vectors.flatMap((vector) =>
      vector.targets.flatMap((target) =>
        Object.keys(set.variants).map((variant) => {
          const state =
            vector.kind === 'text'
              ? 'not-run'
              : vector.skip?.target === target || vector.observable?.[target] === false
                ? 'skip'
                : 'pass';
          return {
            id: vector.id,
            target,
            variant,
            transport: 'cdp',
            via: vector.control.via,
            control: state,
            guarded: state,
            live: state,
          };
        })
      )
    );
    return { ...summary({ set, results: rows, guard: 'slicc-cdp', sha }), observerSkipped: 0 };
  };
  const row = (type) => ({
    type,
    name: 'fixed behaviour',
    file: testFile,
    failureType: type === 'test:fail' ? 'testCodeFailure' : undefined,
  });
  const execution = (type) => ({
    status: type === 'test:pass' ? 'PASS' : 'FAIL',
    files: [testFile],
    rows: [row(type)],
    attempts: [{ exit: type === 'test:pass' ? 0 : 1, files: [testFile], rows: [row(type)] }],
  });
  const inventory = JSON.parse(
    readFileSync(new URL('../../scripts/cert/inventory.json', import.meta.url), 'utf8')
  );
  return {
    plan: plan,
    registry: document('registry', {
      sha256: hash,
      gitHead: commit,
      provenance: {
        workflow: { ref: 'refs/heads/main', path: '.github/workflows/release.yml' },
        subject: { digest: { sha512: 'f'.repeat(128) } },
      },
      signatures: { exit: 0, stdout: '1 package has a verified attestation' },
      ancestry: 'behind',
      crates: ['slicc-cdp-client', 'slicc-discovery', 'slicc-cdp', 'curlwright'].map((crate) => ({
        crate,
        status: 'PASS',
      })),
    }),
    rebuild: document('rebuild', {
      sha256: hash,
      files: ['bin/playwright-cli.wasm', 'bin/curlwright.wasm'].map((file) => ({
        file,
        built: hash,
        published: hash,
      })),
    }),
    guard: document('guard', {
      sha256: hash,
      expected: [],
      failures: [],
      executions: {
        positive: {
          ...execution('test:pass'),
          report: report(commit),
          version: plan.version,
          commit,
        },
        negative: {
          ...execution('test:pass'),
          report: report(previous),
          version: plan.baseline.version,
          commit: previous,
        },
      },
    }),
    integration: document('integration', {
      sha256: hash,
      executions: { positive: execution('test:pass'), negative: execution('test:fail') },
      fixes: [
        {
          commit: fix,
          status: 'PASS',
          results: [{ current: [row('test:pass')], previous: [row('test:fail')] }],
        },
      ],
    }),
    'assets-linux': document('assets-linux', {
      checksums: {
        files: inventory.map((file) => ({ file, status: 'PASS', sha256: hash, expected: hash })),
      },
    }),
    'assets-macos': document('assets-macos', {
      files: inventory
        .filter((file) => file.includes('-darwin-'))
        .map((file) => ({
          file,
          signature: { display: { exit: 0 }, strict: { exit: 0 }, notarisation: { exit: 0 } },
        })),
    }),
  };
}
