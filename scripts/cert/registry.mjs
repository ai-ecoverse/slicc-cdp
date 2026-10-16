import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import {
  capture,
  cleanEnv,
  digest,
  download,
  fetchJSON,
  gh,
  name,
  repo,
  root,
  run,
  unpackPackage,
} from './common.mjs';

export async function registry(plan) {
  const work = path.join(root, '.cert/record');
  const packed = unpackPackage(plan.version, work);
  const metadata = JSON.parse(
    run('npm', ['view', `${name}@${plan.version}`, '--json'], { env: cleanEnv() })
  );
  const failures = [];
  const require = (condition, message) => {
    if (!condition) failures.push(message);
  };
  require(packed.manifest.name === name &&
    packed.manifest.version === plan.version, 'npm manifest identity mismatch');
  require((packed.manifest.gitHead === undefined || packed.manifest.gitHead === plan.commit) &&
    metadata.gitHead === plan.commit, 'gitHead differs from tag');
  require(metadata.dist.integrity ===
    `sha512-${digest(packed.tarball, 'sha512', 'base64')}`, 'npm integrity differs from tarball');
  const url = metadata.dist.attestations?.url;
  require(typeof url === 'string' &&
    new URL(url).origin === 'https://registry.npmjs.org', 'missing npm attestation URL');
  let statement;
  if (url && new URL(url).origin === 'https://registry.npmjs.org') {
    const attestations = await fetchJSON(url);
    const slsa = attestations.attestations?.find(
      (attestation) => attestation.predicateType === 'https://slsa.dev/provenance/v1'
    );
    if (slsa)
      statement = JSON.parse(
        Buffer.from(slsa.bundle.dsseEnvelope.payload, 'base64').toString('utf8')
      );
  }
  const definition = statement?.predicate?.buildDefinition;
  const workflow = definition?.externalParameters?.workflow;
  const provenanceCommit = definition?.resolvedDependencies?.find(
    (dependency) => dependency.digest?.gitCommit
  )?.digest?.gitCommit;
  const subject = statement?.subject?.find(
    (item) => item.name === `pkg:npm/%40ai-ecoverse/slicc-cdp@${plan.version}`
  );
  const builder = statement?.predicate?.runDetails?.builder?.id;
  require(subject?.digest?.sha512 === digest(packed.tarball, 'sha512'), 'SLSA subject mismatch');
  require(workflow?.repository === `https://github.com/${repo}` &&
    workflow?.path === '.github/workflows/release.yml' &&
    workflow?.ref === 'refs/heads/main', 'SLSA workflow is not main release.yml');
  require(provenanceCommit === plan.commit, 'SLSA commit differs from tag');
  require(builder ===
    'https://github.com/actions/runner/github-hosted', 'SLSA builder is not GitHub hosted');
  const ancestry = gh(`repos/${repo}/compare/main...${plan.commit}`).status;
  require(['identical', 'behind'].includes(ancestry), 'tag is not on main');
  const audit = path.join(work, 'audit');
  mkdirSync(audit, { recursive: true });
  writeFileSync(path.join(audit, 'package.json'), '{"private":true}\n');
  run(
    'npm',
    ['install', '--ignore-scripts', '--no-audit', '--no-fund', `${name}@${plan.version}`],
    {
      cwd: audit,
      env: cleanEnv(),
      timeout: 300000,
    }
  );
  const signatures = capture('npm', ['audit', 'signatures'], {
    cwd: audit,
    env: cleanEnv(),
    timeout: 300000,
  });
  require(signatures.exit === 0 &&
    /verified attestation/.test(
      signatures.stdout
    ), 'npm attestation signature verification failed');
  const crates = [];
  for (const [crate, directory] of [
    ['slicc-cdp-client', 'cdp-client'],
    ['slicc-discovery', 'discovery'],
    ['slicc-cdp', 'playwright-cli'],
    ['curlwright', 'curlwright'],
  ]) {
    const metadata = (await fetchJSON(`https://crates.io/api/v1/crates/${crate}/${plan.version}`))
      .version;
    const file = path.join(work, `${crate}.crate`);
    await download(`https://static.crates.io/crates/${crate}/${crate}-${plan.version}.crate`, file);
    const prefix = `${crate}-${plan.version}`;
    const original = run('tar', ['-xzf', file, '-O', `${prefix}/Cargo.toml.orig`]);
    const expected = run('git', ['show', `${plan.commit}:crates/${directory}/Cargo.toml`], {
      cwd: root,
    }).replaceAll('version = "0.0.0"', `version = "${plan.version}"`);
    const vcs = JSON.parse(run('tar', ['-xzf', file, '-O', `${prefix}/.cargo_vcs_info.json`]));
    const checksum = digest(file);
    const sourceFiles = run(
      'git',
      ['ls-tree', '-r', '--name-only', plan.commit, `crates/${directory}`],
      { cwd: root }
    )
      .split('\n')
      .filter((source) => source.includes('/src/') || source.endsWith('/build.rs'));
    const different = sourceFiles.filter(
      (source) =>
        run('tar', [
          '-xzf',
          file,
          '-O',
          `${prefix}/${source.slice(`crates/${directory}/`.length)}`,
        ]) !== run('git', ['show', `${plan.commit}:${source}`], { cwd: root })
    );
    const valid =
      metadata.num === plan.version &&
      !metadata.yanked &&
      metadata.checksum === checksum &&
      metadata.repository === `https://github.com/${repo}` &&
      vcs.git.sha1 === plan.commit &&
      original === expected &&
      !different.length;
    require(valid, `${crate} version/source/checksum does not match tag`);
    crates.push({
      crate,
      version: metadata.num,
      sha256: checksum,
      gitHead: vcs.git.sha1,
      different,
      status: valid ? 'PASS' : 'FAIL',
    });
  }
  return {
    status: failures.length ? 'FAIL' : 'PASS',
    failures,
    sha256: packed.sha256,
    gitHead: metadata.gitHead,
    provenance: { workflow, commit: provenanceCommit, builder, subject },
    signatures,
    ancestry,
    crates,
  };
}
