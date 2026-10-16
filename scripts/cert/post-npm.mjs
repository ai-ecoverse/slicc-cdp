import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import {
  capture,
  cleanEnv,
  digest,
  evidence,
  fetchJSON,
  gh,
  name,
  repo,
  root,
  run,
  unpackPackage,
} from './common.mjs';

await evidence('registry', async (plan) => {
  const failures = [];
  const incomplete = [];
  const check = (condition, message, available = true) => {
    if (!available) incomplete.push(message);
    else if (!condition) failures.push(message);
  };
  const facts = {};
  try {
    const work = path.join(root, '.cert/post-npm');
    const packed = unpackPackage(plan.version, work);
    const metadata = JSON.parse(
      run('npm', ['view', `${name}@${plan.version}`, '--json', '--prefer-online'], {
        env: cleanEnv(),
      })
    );
    Object.assign(facts, {
      sha256: packed.sha256,
      sha512: digest(packed.tarball, 'sha512'),
      manifest: packed.manifest,
      metadata,
    });
    check(
      metadata.name === name &&
        metadata.version === plan.version &&
        packed.manifest.name === name &&
        packed.manifest.version === plan.version,
      'npm package identity mismatch'
    );
    check(metadata.gitHead === plan.commit, 'gitHead differs from tag', !!metadata.gitHead);
    if (packed.manifest.gitHead !== undefined)
      check(packed.manifest.gitHead === plan.commit, 'packed gitHead differs from tag');
    check(
      metadata.dist?.integrity === `sha512-${digest(packed.tarball, 'sha512', 'base64')}`,
      'npm integrity differs from tarball',
      !!metadata.dist?.integrity
    );
    let tag = gh(`repos/${repo}/git/ref/tags/v${plan.version}`).object;
    for (let depth = 0; tag.type === 'tag' && depth < 10; depth++)
      tag = gh(`repos/${repo}/git/tags/${tag.sha}`).object;
    facts.tag = tag;
    check(
      tag.type === 'commit' && tag.sha === plan.commit,
      'tag changed between plan and verification'
    );
    const url = metadata.dist?.attestations?.url;
    let origin;
    try {
      origin = new URL(url).origin;
    } catch {
      origin = null;
    }
    check(origin === 'https://registry.npmjs.org', 'npm attestation URL absent or invalid', !!url);
    if (origin === 'https://registry.npmjs.org') {
      const attestations = await fetchJSON(url);
      const attestation = attestations.attestations?.find(
        (item) => item.predicateType === 'https://slsa.dev/provenance/v1'
      );
      if (!attestation?.bundle?.dsseEnvelope?.payload) incomplete.push('SLSA payload unavailable');
      else
        facts.statement = JSON.parse(
          Buffer.from(attestation.bundle.dsseEnvelope.payload, 'base64').toString('utf8')
        );
    }
    const statement = facts.statement;
    const definition = statement?.predicate?.buildDefinition;
    const workflow = definition?.externalParameters?.workflow;
    const commit = definition?.resolvedDependencies?.find((item) => item.digest?.gitCommit)?.digest
      .gitCommit;
    const subject = statement?.subject?.find(
      (item) => item.name === `pkg:npm/%40ai-ecoverse/slicc-cdp@${plan.version}`
    );
    const builder = statement?.predicate?.runDetails?.builder?.id;
    check(
      statement?.predicateType === 'https://slsa.dev/provenance/v1',
      'SLSA v1 predicate missing or wrong',
      !!statement?.predicateType
    );
    check(
      subject?.digest?.sha512 === facts.sha512,
      'SLSA subject differs from registry bytes',
      !!subject?.digest?.sha512
    );
    check(
      workflow?.repository === `https://github.com/${repo}` &&
        workflow?.path === '.github/workflows/release.yml' &&
        workflow?.ref === 'refs/heads/main',
      'SLSA workflow is not main release.yml',
      !!(workflow?.repository && workflow?.path && workflow?.ref)
    );
    check(commit === plan.commit, 'SLSA gitCommit differs from tag/gitHead', !!commit);
    check(
      builder === 'https://github.com/actions/runner/github-hosted',
      'SLSA builder is not GitHub hosted',
      !!builder
    );
    facts.ancestry = gh(`repos/${repo}/compare/main...${plan.commit}`).status;
    check(['identical', 'behind'].includes(facts.ancestry), 'tag is not on main', !!facts.ancestry);
    const audit = path.join(work, 'audit');
    mkdirSync(audit, { recursive: true });
    writeFileSync(path.join(audit, 'package.json'), '{"private":true}\n');
    run(
      'npm',
      ['install', '--ignore-scripts', '--no-audit', '--no-fund', `${name}@${plan.version}`],
      { cwd: audit, env: cleanEnv(), timeout: 300000 }
    );
    facts.signatures = capture('npm', ['audit', 'signatures'], {
      cwd: audit,
      env: cleanEnv(),
      timeout: 300000,
    });
    const completed =
      Number.isInteger(facts.signatures.exit) &&
      !facts.signatures.signal &&
      !facts.signatures.error;
    check(facts.signatures.exit === 0, 'npm signature verification failed', completed);
    check(
      /verified attestation/.test(facts.signatures.stdout),
      'npm audit verified no attestation',
      completed && facts.signatures.exit === 0
    );
  } catch (error) {
    incomplete.push(error.message);
  }
  return {
    ...facts,
    status: failures.length ? 'FAIL' : incomplete.length ? 'INCOMPLETE' : 'PASS',
    failures,
    incomplete,
    reason: [...failures, ...incomplete].join('; '),
  };
});
