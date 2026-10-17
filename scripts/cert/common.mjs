import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { parseArgs } from 'node:util';

export const name = '@ai-ecoverse/slicc-cdp';
export const repo = 'ai-ecoverse/slicc-cdp';
export const semver = /^\d+\.\d+\.\d+$/;
export const root = path.resolve(import.meta.dirname, '../..');
export const json = (file) => JSON.parse(readFileSync(file, 'utf8'));
export const save = (file, data) => {
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, `${JSON.stringify(data, null, 2)}\n`);
};
export const digest = (file, algorithm = 'sha256', encoding = 'hex') =>
  createHash(algorithm).update(readFileSync(file)).digest(encoding);
export const cleanEnv = () => {
  const env = { ...process.env };
  for (const key of Object.keys(env))
    if (
      /TOKEN|SECRET|PASSWORD|CREDENTIAL|API_KEY|SLICC_CDP_URL|NODE_TEST_CONTEXT|SSH_AUTH_SOCK|GITHUB_(ENV|OUTPUT|PATH|STATE|STEP_SUMMARY)$/.test(
        key
      )
    )
      delete env[key];
  env.LEFTHOOK = '0';
  return env;
};
export const run = (command, args, options = {}) =>
  execFileSync(command, args, {
    encoding: 'utf8',
    timeout: 120000,
    maxBuffer: 32 * 1024 * 1024,
    ...options,
  }).trim();
export const gh = (endpoint) => JSON.parse(run('gh', ['api', endpoint]));
export function args(extra = {}) {
  return parseArgs({
    options: {
      plan: { type: 'string' },
      out: { type: 'string' },
      ...extra,
    },
  }).values;
}
export function identity(plan) {
  return {
    version: plan.version,
    mode: plan.mode ?? 'release',
    legacyChecksum: plan.legacyChecksum ?? false,
    commit: plan.commit,
    baseline: plan.baseline,
    harness: plan.harness,
    rust: plan.rust,
    vectors: plan.vectors,
    run: plan.run,
    attempt: plan.attempt,
    ...(plan.mode === 'pr'
      ? {
          repository: plan.repository,
          pr: plan.pr,
          head: plan.head,
          base: plan.base,
          tools: plan.tools,
        }
      : {}),
  };
}
export function unpackPackage(version, dir) {
  mkdirSync(dir, { recursive: true });
  const packed = JSON.parse(
    run(
      'npm',
      ['pack', `${name}@${version}`, '--ignore-scripts', '--json', '--pack-destination', dir],
      {
        env: cleanEnv(),
      }
    )
  );
  if (packed.length !== 1 || !/^[a-zA-Z0-9.-]+\.tgz$/.test(packed[0].filename))
    throw new Error('npm pack returned an unexpected tarball');
  const tarball = path.join(dir, packed[0].filename);
  run('tar', ['-xzf', tarball, '-C', dir]);
  return {
    tarball,
    manifest: json(path.join(dir, 'package/package.json')),
    sha256: digest(tarball),
  };
}
export function tree(commit, dir) {
  mkdirSync(dir, { recursive: true });
  const archive = path.join(dir, 'source.tar');
  run('git', ['archive', '--format=tar', '-o', archive, commit], { cwd: root });
  run('tar', ['-xf', 'source.tar'], { cwd: dir });
  return dir;
}
export async function fetchJSON(url) {
  const response = await fetch(url, {
    headers: { 'User-Agent': 'slicc-cdp-cert' },
    signal: AbortSignal.timeout(60000),
  });
  if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
  return response.json();
}
export async function download(url, file) {
  const response = await fetch(url, {
    headers: { 'User-Agent': 'slicc-cdp-cert' },
    signal: AbortSignal.timeout(120000),
  });
  if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
  writeFileSync(file, Buffer.from(await response.arrayBuffer()));
}
export function capture(command, argv, options = {}) {
  const result = spawnSync(command, argv, {
    encoding: 'utf8',
    timeout: 120000,
    maxBuffer: 32 * 1024 * 1024,
    ...options,
  });
  return {
    exit: result.status,
    signal: result.signal,
    error: result.error?.message,
    stdout: result.stdout ?? '',
    stderr: result.stderr ?? '',
  };
}
export async function evidence(which, execute) {
  const options = args();
  const plan = json(options.plan);
  let result;
  try {
    result = await execute(plan, options);
  } catch (error) {
    result = {
      status: error.certStatus === 'FAIL' ? 'FAIL' : 'INCOMPLETE',
      reason: error.stack ?? String(error),
    };
  }
  save(options.out, { ...identity(plan), which, ...result });
  console.log(`${which}: ${result.status} ${options.out}`);
}
