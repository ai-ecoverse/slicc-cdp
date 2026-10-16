import { appendFileSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { args, gh, identity, json, repo, root, run, save, semver } from './common.mjs';

const options = args({ version: { type: 'string' } });
if (!semver.test(options.version ?? '')) throw new Error('--version must be x.y.z');
run('git', ['fetch', 'origin', 'main', '--tags'], { cwd: root });
const commit = run('git', ['rev-parse', `refs/tags/v${options.version}^{commit}`], { cwd: root });
run('git', ['merge-base', '--is-ancestor', commit, 'origin/main'], { cwd: root });
const seed = json(path.join(root, 'scripts/cert/base.json'));
let baseline = seed;
const previous = run('git', ['tag', '--list', 'v*'], { cwd: root })
  .split('\n')
  .map((tag) => tag.slice(1))
  .filter((version) => semver.test(version))
  .sort((a, b) => {
    const x = a.split('.').map(Number);
    const y = b.split('.').map(Number);
    return x[0] - y[0] || x[1] - y[1] || x[2] - y[2];
  });
const index = previous.indexOf(options.version);
for (const version of previous.slice(0, index).reverse()) {
  const sha = run('git', ['rev-parse', `v${version}^{commit}`], { cwd: root });
  const statuses = gh(`repos/${repo}/commits/${sha}/statuses?per_page=100`);
  const status = statuses.find((entry) => entry.context === 'cert-review');
  if (status?.state !== 'success') continue;
  const release = gh(`repos/${repo}/releases/tags/v${version}`);
  const line = release.body
    ?.split('\n')
    .find((entry) => entry.startsWith(`certified-candidate @ai-ecoverse/slicc-cdp@${version} `));
  const hash = line?.match(/ sha256=([a-f0-9]{64}) /)?.[1];
  if (!hash || !line.includes(`gitHead=${sha} `)) continue;
  baseline = { version, commit: sha, sha256: hash, source: status.target_url };
  break;
}
if (!previous.slice(0, index).includes(baseline.version))
  throw new Error('no earlier certified baseline exists');
baseline = {
  ...baseline,
  commit: run('git', ['rev-parse', `v${baseline.version}^{commit}`], { cwd: root }),
};
if (!baseline.commit.startsWith(seed.commit) && baseline.version === seed.version)
  throw new Error('seed tag changed');
const manifest = JSON.parse(run('git', ['show', `${commit}:package.json`], { cwd: root }));
const pins = json(path.join(root, 'package.json'));
const historical = json(path.join(root, 'scripts/cert/toolchains.json'))[options.version];
let rust;
let rustSource = historical?.source ?? 'tag:rust-toolchain.toml';
try {
  rust = run('git', ['show', `${commit}:rust-toolchain.toml`], { cwd: root }).match(
    /^channel = "(\d+\.\d+\.\d+)"$/m
  )?.[1];
} catch {
  rust = historical?.rust;
}
if (!rust) {
  try {
    const runs = gh(
      `repos/${repo}/actions/runs?head_sha=${commit}&status=success&per_page=100`
    ).workflow_runs;
    const release = runs.find(
      (entry) =>
        entry.name === 'Release' && entry.head_branch === 'main' && entry.head_sha === commit
    );
    if (release) {
      const jobs = gh(`repos/${repo}/actions/runs/${release.id}/jobs?per_page=100`).jobs;
      const job = jobs.find((entry) => entry.name === 'release');
      if (job) {
        const log = run('gh', [
          'run',
          'view',
          String(release.id),
          '--repo',
          repo,
          '--job',
          String(job.id),
          '--log',
        ]);
        const versions = [
          ...new Set([...log.matchAll(/rustc (\d+\.\d+\.\d+) \(/g)].map((match) => match[1])),
        ];
        if (versions.length === 1) {
          rust = versions[0];
          rustSource = release.html_url;
        }
      }
    }
  } catch {
    rustSource = 'exact release compiler log unavailable';
  }
}
const issues = rust ? [] : ['release has no recorded exact Rust toolchain'];
const declared = JSON.parse(
  run(
    process.execPath,
    [
      path.join(root, 'scripts/cert/proofs.mjs'),
      '--base',
      baseline.commit,
      '--head',
      commit,
      '--out',
      path.join(path.dirname(options.out), 'derived-proofs.json'),
    ],
    { cwd: root }
  )
);
const fixes = declared.fixes;
const plan = {
  version: options.version,
  commit,
  baseline,
  harness: run('git', ['rev-parse', 'HEAD'], { cwd: root }),
  rust: rust ?? null,
  rustSource,
  issues,
  vectors: pins.devDependencies['@ai-ecoverse/slicc-browsers'],
  chrome: manifest.config.chrome,
  kernel: manifest.devDependencies['@ai-ecoverse/slicc-kernel'],
  run: process.env.GITHUB_RUN_ID ?? 'local',
  attempt: process.env.GITHUB_RUN_ATTEMPT ?? '1',
  fixes,
  proofs: declared,
};
if (!semver.test(plan.vectors ?? '')) throw new Error('shared guard runner must be exactly pinned');
save(options.out, plan);
if (process.env.GITHUB_OUTPUT)
  appendFileSync(process.env.GITHUB_OUTPUT, `version=${plan.version}\nrust=${plan.rust ?? ''}\n`);
console.log(JSON.stringify(identity(plan)));
console.log(readFileSync(options.out, 'utf8'));
