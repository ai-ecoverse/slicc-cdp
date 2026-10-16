import { chmodSync, copyFileSync, cpSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { cleanEnv, json, repo, root, run, save, tree, unpackPackage } from './common.mjs';

export async function stage(plan, version, commit, label, testsFrom) {
  const dir = path.join(root, '.cert', label);
  const source = tree(commit, path.join(dir, 'tree'));
  if (testsFrom)
    cpSync(path.join(testsFrom, 'test'), path.join(source, 'test'), { recursive: true });
  const packed = unpackPackage(version, path.join(dir, 'registry'));
  if (version === plan.baseline.version && packed.sha256 !== plan.baseline.sha256)
    throw new Error('baseline published hash differs from certified record');
  cpSync(path.join(dir, 'registry/package/src'), path.join(source, 'src'), { recursive: true });
  const pkg = json(path.join(source, 'package.json'));
  pkg.version = version;
  if (testsFrom) {
    const harness = json(path.join(testsFrom, 'package.json'));
    Object.assign(pkg, harness, { version });
    cpSync(path.join(testsFrom, 'packages'), path.join(source, 'packages'), { recursive: true });
    copyFileSync(path.join(testsFrom, 'package-lock.json'), path.join(source, 'package-lock.json'));
    const lock = json(path.join(source, 'package-lock.json'));
    lock.version = version;
    lock.packages[''].version = version;
    save(path.join(source, 'package-lock.json'), lock);
  }
  save(path.join(source, 'package.json'), pkg);
  const env = {
    ...cleanEnv(),
    RUSTUP_TOOLCHAIN: plan.rust,
    SLICC_FIXTURE_BROWSER_API: pathToFileURL(path.join(root, 'src/browser.js')).href,
    CARGO_TARGET_DIR: path.join(source, 'target'),
  };
  run('npm', ['ci', '--ignore-scripts', '--no-audit', '--no-fund'], {
    cwd: source,
    env,
    timeout: 300000,
  });
  const chrome = run(
    process.execPath,
    [path.join(root, 'src/cli.js'), 'install-browser', '--version', plan.chrome],
    {
      env,
      timeout: 300000,
    }
  );
  env.CHROMIUM_EXECUTABLE = chrome.split('\n').at(-1);
  mkdirSync(path.join(source, 'target/debug'), { recursive: true });
  mkdirSync(path.join(source, 'target/wasm32-wasip1/debug'), { recursive: true });
  const arch = process.arch === 'arm64' ? 'arm64' : 'amd64';
  for (const command of ['playwright-cli', 'curlwright']) {
    const binary = path.join(source, 'target/debug', command);
    const asset = `${command}-linux-${arch}`;
    run(
      'gh',
      [
        'release',
        'download',
        `v${version}`,
        '--repo',
        repo,
        '--pattern',
        asset,
        '--dir',
        dir,
        '--clobber',
      ],
      { timeout: 300000 }
    );
    copyFileSync(path.join(dir, asset), binary);
    chmodSync(binary, 0o755);
    copyFileSync(
      path.join(dir, 'registry/package/bin', `${command}.wasm`),
      path.join(source, 'target/wasm32-wasip1/debug', `${command}.wasm`)
    );
  }
  return { source, env, sha256: packed.sha256 };
}
