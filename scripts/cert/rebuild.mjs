import { copyFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import {
  cleanEnv,
  digest,
  evidence,
  json,
  root,
  run,
  save,
  tree,
  unpackPackage,
} from './common.mjs';
import { unpackLocal } from './pr-subject.mjs';

await evidence('rebuild', async (plan) => {
  const work = path.join(root, '.cert/rebuild');
  const source = tree(plan.commit, path.join(work, 'tree'));
  const packed =
    plan.mode === 'pr'
      ? unpackLocal(plan, plan.version, path.join(work, 'registry'))
      : unpackPackage(plan.version, path.join(work, 'registry'));
  const pkg = json(path.join(source, 'package.json'));
  pkg.version = plan.version;
  save(path.join(source, 'package.json'), pkg);
  const env = cleanEnv();
  env.CARGO_TARGET_DIR = path.join(source, 'target');
  const toolchain = run('rustc', [`+${plan.rust}`, '--version', '--verbose'], { env });
  run(
    'cargo',
    [
      `+${plan.rust}`,
      'build',
      '--locked',
      '--release',
      '--target',
      'wasm32-wasip1',
      '-p',
      'slicc-cdp',
      '-p',
      'curlwright',
    ],
    {
      cwd: source,
      env,
      timeout: 1200000,
    }
  );
  const files = ['playwright-cli.wasm', 'curlwright.wasm'].map((file) => {
    const built = path.join(source, 'target/wasm32-wasip1/release', file);
    const published = path.join(work, 'registry/package/bin', file);
    return { file: `bin/${file}`, built: digest(built), published: digest(published) };
  });
  mkdirSync(path.join(work, 'published'), { recursive: true });
  for (const file of files)
    copyFileSync(
      path.join(work, 'registry/package', file.file),
      path.join(work, 'published', path.basename(file.file))
    );
  return {
    status: files.every((file) => file.built === file.published) ? 'PASS' : 'FAIL',
    reason: files.every((file) => file.built === file.published)
      ? undefined
      : 'wasm differs from published bytes',
    sha256: packed.sha256,
    toolchain,
    lockSha256: digest(path.join(source, 'Cargo.lock')),
    files,
  };
});
