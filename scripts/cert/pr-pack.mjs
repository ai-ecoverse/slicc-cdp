import { copyFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import { args, cleanEnv, digest, json, root, run, save, tree } from './common.mjs';
import { prIdentity } from './pr-common.mjs';

const options = args({
  side: { type: 'string' },
  target: { type: 'string', default: 'wasm32-wasip1' },
  label: { type: 'string', default: 'pack' },
});
const plan = json(options.plan);
if (plan.mode !== 'pr' || !['candidate', 'base'].includes(options.side))
  throw new Error('invalid PR pack side');
const subject =
  options.side === 'candidate' ? { version: plan.version, commit: plan.commit } : plan.baseline;
if (
  !/^[a-f0-9]{40}$/.test(subject.commit) ||
  !/^[a-zA-Z0-9-]+$/.test(options.target) ||
  !/^[a-zA-Z0-9-]+$/.test(options.label)
)
  throw new Error('invalid build identity');
const source = tree(subject.commit, path.join(root, '.cert/build', options.side, options.target));
const pkg = json(path.join(source, 'package.json'));
Object.assign(pkg, { version: subject.version, gitHead: subject.commit });
save(path.join(source, 'package.json'), pkg);
const lock = json(path.join(source, 'package-lock.json'));
lock.version = subject.version;
lock.packages[''].version = subject.version;
save(path.join(source, 'package-lock.json'), lock);
const env = {
  ...cleanEnv(),
  RUSTUP_TOOLCHAIN: plan.rust,
  CARGO_TARGET_DIR: path.join(source, 'target'),
};
mkdirSync(options.out, { recursive: true });
const tool = options.target === 'aarch64-unknown-linux-musl' ? 'cross' : 'cargo';
run(
  tool,
  [
    'build',
    '--locked',
    '--release',
    '--target',
    options.target,
    '-p',
    'slicc-cdp',
    '-p',
    'curlwright',
  ],
  { cwd: source, env, timeout: 1800000 }
);
const files = [];
const wasm = options.target === 'wasm32-wasip1';
for (const command of ['playwright-cli', 'curlwright']) {
  const extension = wasm ? '.wasm' : options.target.includes('windows') ? '.exe' : '';
  const built = path.join(source, 'target', options.target, 'release', `${command}${extension}`);
  if (wasm) {
    mkdirSync(path.join(source, 'bin'), { recursive: true });
    copyFileSync(built, path.join(source, 'bin', `${command}.wasm`));
  } else {
    const file = `${command}-${options.label}${extension}`;
    copyFileSync(built, path.join(options.out, file));
    files.push({ file, sha256: digest(path.join(options.out, file)) });
  }
}
if (wasm) {
  run('cargo', ['build', '--locked', '--release', '-p', 'slicc-cdp', '-p', 'curlwright'], {
    cwd: source,
    env,
    timeout: 1800000,
  });
  for (const command of ['playwright-cli', 'curlwright']) {
    const file = `${command}-host`;
    copyFileSync(path.join(source, 'target/release', command), path.join(options.out, file));
    files.push({ file, sha256: digest(path.join(options.out, file)) });
  }
  const packed = JSON.parse(
    run(
      'npm',
      ['pack', '--ignore-scripts', '--json', '--pack-destination', path.resolve(options.out)],
      { cwd: source, env }
    )
  );
  if (packed.length !== 1 || !/^[\w.-]+\.tgz$/.test(packed[0].filename))
    throw new Error('unexpected npm pack output');
  files.push({
    file: packed[0].filename,
    sha256: digest(path.join(options.out, packed[0].filename)),
    package: true,
  });
  for (const file of ['install.sh', 'LICENSE'])
    copyFileSync(path.join(source, file), path.join(options.out, file));
  run(
    'bash',
    [
      path.join(root, 'scripts/generate-notices.sh'),
      path.resolve(options.out, 'THIRD-PARTY-NOTICES'),
    ],
    { cwd: source, env, timeout: 300000 }
  );
  for (const file of ['install.sh', 'LICENSE', 'THIRD-PARTY-NOTICES'])
    files.push({ file, sha256: digest(path.join(options.out, file)) });
}
save(path.join(options.out, 'build.json'), {
  ...prIdentity(plan),
  side: options.side,
  subject,
  target: options.target,
  label: options.label,
  toolchain: run('rustc', ['--version', '--verbose'], { env }),
  lockSha256: digest(path.join(source, 'Cargo.lock')),
  files,
});
