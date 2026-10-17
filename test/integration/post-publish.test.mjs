import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const inventory = JSON.parse(readFileSync(path.join(root, 'scripts/cert/inventory.json')));
const save = (file, data) => {
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, JSON.stringify(data));
};
function fixture(t) {
  const directory = mkdtempSync(path.join(tmpdir(), 'slicc-post-publish-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const commit = 'b'.repeat(40),
    sha256 = 'a'.repeat(64),
    sha512 = 'f'.repeat(128);
  const plan = {
    version: '1.26.3',
    mode: 'release',
    legacyChecksum: false,
    postPublish: true,
    commit,
    harness: commit,
    run: '42',
    attempt: '1',
  };
  const document = (which, data) => ({ ...plan, which, status: 'PASS', ...data });
  const registry = document('registry', {
    sha256,
    sha512,
    tag: { type: 'commit', sha: commit },
    ancestry: 'behind',
    manifest: { name: '@ai-ecoverse/slicc-cdp', version: plan.version },
    metadata: {
      name: '@ai-ecoverse/slicc-cdp',
      version: plan.version,
      gitHead: commit,
      dist: { integrity: `sha512-${Buffer.from(sha512, 'hex').toString('base64')}` },
    },
    statement: {
      predicateType: 'https://slsa.dev/provenance/v1',
      subject: [{ name: `pkg:npm/%40ai-ecoverse/slicc-cdp@${plan.version}`, digest: { sha512 } }],
      predicate: {
        buildDefinition: {
          externalParameters: {
            workflow: {
              repository: 'https://github.com/ai-ecoverse/slicc-cdp',
              path: '.github/workflows/release.yml',
              ref: 'refs/heads/main',
            },
          },
          resolvedDependencies: [{ digest: { gitCommit: commit } }],
        },
        runDetails: { builder: { id: 'https://github.com/actions/runner/github-hosted' } },
      },
    },
    signatures: { exit: 0, stdout: '1 package has a verified attestation', stderr: '' },
  });
  const files = [...inventory, 'SHA256SUMS'].map((file) => ({
    file,
    sha256,
    published: `sha256:${sha256}`,
    status: 'PASS',
  }));
  const linux = document('assets-linux', {
    files,
    checksums: {
      status: 'PASS',
      files: inventory.map((file) => ({ file, sha256, expected: sha256, status: 'PASS' })),
    },
  });
  const macos = document('assets-macos', {
    files: files
      .filter((item) => item.file.includes('-darwin-'))
      .map((item) => ({
        ...item,
        signature: {
          display: {
            exit: 0,
            stderr: `Identifier=${item.file.startsWith('playwright-cli') ? 'ai.sliccy.playwright-cli' : 'ai.sliccy.curlwright'}\nAuthority=Developer ID Application: Example\nTeamIdentifier=S8LB56P782\n`,
          },
          strict: { exit: 0, stderr: '' },
          notarisation: { exit: 0, stderr: 'accepted\nsource=Notarized Developer ID\n' },
        },
      })),
  });
  return { directory, plan, registry, linux, macos };
}
function record(f) {
  for (const [file, data] of Object.entries({
    plan: f.plan,
    registry: f.registry,
    'assets-linux': f.linux,
    'assets-macos': f.macos,
  }))
    save(path.join(f.directory, `${file}.json`), data);
  const result = spawnSync(
    process.execPath,
    [
      path.join(root, 'scripts/cert/post-record.mjs'),
      '--evidence',
      f.directory,
      '--out',
      path.join(f.directory, 'record'),
    ],
    {
      encoding: 'utf8',
      env: { ...process.env, CERT_UPSTREAM: '{"plan":"success","assets":"success"}' },
    }
  );
  assert.equal(result.status, 0, result.stderr);
  return JSON.parse(readFileSync(path.join(f.directory, 'record/record.json')));
}
test('post-publish public record CLI recomputes full supply-chain facts without behaviour claims', (t) => {
  const output = record(fixture(t));
  assert.equal(output.status, 'PASS');
  assert.match(output.line, /^verified @ai-ecoverse\/slicc-cdp@1.26.3 sha256=[a-f0-9]{64}/);
  assert.doesNotMatch(output.line, /^certified(?:-candidate)? /);
  assert.match(output.line, /macOS=4 signed\+notarised; checksums=complete/);
  assert.match(output.line, /pr-certified: not asserted/);
});
for (const scenario of [
  {
    name: 'missing SLSA',
    mutate: (f) => {
      delete f.registry.statement;
    },
    status: 'INCOMPLETE',
  },
  {
    name: 'missing checksum inventory',
    mutate: (f) => {
      f.linux.checksums.files.pop();
    },
    status: 'INCOMPLETE',
  },
  {
    name: 'missing Mac execution',
    mutate: (f) => {
      f.macos.files[0].signature.strict.exit = null;
    },
    status: 'INCOMPLETE',
  },
  {
    name: 'subject mismatch',
    mutate: (f) => {
      f.registry.statement.subject[0].digest.sha512 = '0'.repeat(128);
    },
    status: 'FAIL',
  },
  {
    name: 'wrong workflow',
    mutate: (f) => {
      f.registry.statement.predicate.buildDefinition.externalParameters.workflow.ref =
        'refs/heads/other';
    },
    status: 'FAIL',
  },
  {
    name: 'wrong native bytes and missing SLSA',
    mutate: (f) => {
      delete f.registry.statement;
      f.macos.files[0].sha256 = '0'.repeat(64);
    },
    status: 'FAIL',
  },
  {
    name: 'wrong notarisation',
    mutate: (f) => {
      f.macos.files[0].signature.notarisation.stderr = 'rejected';
    },
    status: 'FAIL',
  },
])
  test(`post-publish ${scenario.name}`, (t) => {
    const f = fixture(t);
    scenario.mutate(f);
    const output = record(f);
    assert.equal(output.status, scenario.status);
    assert.equal(output.certified, false);
    if (scenario.status === 'FAIL') assert.match(output.line, /supply-chain alert/);
  });

test('release publishes before code-only verification and cert jobs avoid developer browser ports', () => {
  const release = readFileSync(path.join(root, '.github/workflows/release.yml'), 'utf8');
  assert.doesNotMatch(release, /uses: \.\/\.github\/workflows\/cert-review.yml/);
  assert.match(release, /needs: \[release, upload-assets, verify-signatures\]/);
  assert.match(release, /needs: \[release, publish-release\]/);
  const verify = readFileSync(path.join(root, '.github/workflows/verify-release.yml'), 'utf8');
  assert.match(verify, /workflow_dispatch:/);
  assert.doesNotMatch(verify, /cloud-run-bench|secrets:|id-token:|claude/);
  for (const file of [
    'release.yml',
    'verify-release.yml',
    'cert-review.yml',
    'cert-call-proof.yml',
  ])
    assert.match(
      readFileSync(path.join(root, '.github/workflows', file), 'utf8'),
      /SLICC_CDP_PORTS: 9300-9309/
    );
});

test('post-publish notes CLI is idempotent and refuses another hash without a PATCH', (t) => {
  const f = fixture(t);
  const output = record(f);
  const bin = path.join(f.directory, 'bin');
  mkdirSync(bin);
  const release = path.join(f.directory, 'release.json');
  const patches = path.join(f.directory, 'patches');
  writeFileSync(
    path.join(bin, 'gh'),
    `#!/usr/bin/env node\nimport { readFileSync, writeFileSync } from 'node:fs';\nconst args=process.argv.slice(2);\nif(args.includes('PATCH')){const payload=JSON.parse(readFileSync(args[args.indexOf('--input')+1]));const r=JSON.parse(readFileSync(${JSON.stringify(release)}));r.body=payload.body;writeFileSync(${JSON.stringify(release)},JSON.stringify(r));writeFileSync(${JSON.stringify(patches)},'patched');console.log('{}');}\nelse console.log(readFileSync(${JSON.stringify(release)},'utf8'));\n`,
    { mode: 0o755 }
  );
  save(release, { id: 90, tag_name: 'v1.26.3', draft: false, body: 'Original notes' });
  const command = () =>
    execFileSync(
      process.execPath,
      [
        path.join(root, 'scripts/cert/post-notes.mjs'),
        '--record',
        path.join(f.directory, 'record/record.json'),
      ],
      {
        encoding: 'utf8',
        env: { ...process.env, GITHUB_REF: 'refs/heads/main', PATH: `${bin}:${process.env.PATH}` },
      }
    );
  command();
  const first = readFileSync(release, 'utf8');
  rmSync(patches);
  command();
  assert.equal(readFileSync(release, 'utf8'), first);
  for (const prefix of ['certified', 'verified']) {
    const previousLine = output.line.replace(/^verified /, `${prefix} `);
    const body = `Leading notes\n\n<!-- cert-verify -->\n## Existing heading\n\n${previousLine}\n\nKeep marked context\n<!-- /cert-verify -->\nTrailing notes\n`;
    save(release, { id: 90, tag_name: 'v1.26.3', draft: false, body });
    command();
    assert.equal(JSON.parse(readFileSync(release)).body, body.replace(previousLine, output.line));
    const migrated = readFileSync(release, 'utf8');
    rmSync(patches, { force: true });
    command();
    assert.equal(readFileSync(release, 'utf8'), migrated);
    assert.throws(() => readFileSync(patches), { code: 'ENOENT' });
    save(release, {
      id: 90,
      tag_name: 'v1.26.3',
      draft: false,
      body: body.replace(output.sha256, '0'.repeat(64)),
    });
    assert.throws(command, /different bytes/);
    assert.throws(() => readFileSync(patches), { code: 'ENOENT' });
  }
  const literalLine = `${output.line} (literal $& $1 $$)`;
  save(path.join(f.directory, 'record/record.json'), { ...output, line: literalLine });
  const body = `Before\n<!-- cert-verify -->\n${output.line}\n<!-- /cert-verify -->\nAfter\n`;
  save(release, { id: 90, tag_name: 'v1.26.3', draft: false, body });
  command();
  assert.equal(
    JSON.parse(readFileSync(release)).body,
    body.replace(output.line, () => literalLine)
  );
});
