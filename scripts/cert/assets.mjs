import { chmodSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import { capture, digest, evidence, gh, json, repo, root, run } from './common.mjs';
import { prIdentity } from './pr-common.mjs';

function verifySignature(file, asset) {
  chmodSync(file, 0o755);
  const identifier = asset.startsWith('playwright-cli')
    ? 'ai.sliccy.playwright-cli'
    : 'ai.sliccy.curlwright';
  const display = capture('codesign', ['-dv', '--verbose=2', file]);
  const strict = capture('codesign', ['--verify', '--strict', file]);
  const notarisation = capture('spctl', ['-a', '-vv', '-t', 'install', file]);
  const team = process.env.CERT_APPLE_TEAM || 'S8LB56P782';
  const valid =
    display.stderr.includes(`Identifier=${identifier}\n`) &&
    display.stderr.includes('Authority=Developer ID Application:') &&
    display.stderr.includes(`TeamIdentifier=${team}\n`) &&
    display.exit === 0 &&
    strict.exit === 0 &&
    notarisation.exit === 0 &&
    notarisation.stderr.includes('source=Notarized Developer ID') &&
    /\baccepted\b/.test(notarisation.stderr);
  const complete = [display, strict, notarisation].every(
    (result) => result.exit !== null && !result.signal && !result.error
  );
  return {
    valid,
    status: !complete ? 'INCOMPLETE' : valid ? 'PASS' : 'FAIL',
    identifier,
    team,
    display,
    strict,
    notarisation,
  };
}

const checksumMode = (plan) => [
  '--mode',
  plan.mode ?? 'release',
  ...(plan.legacyChecksum ? ['--legacy'] : []),
];

function assetSource(plan) {
  const local = plan.mode === 'pr';
  const dir = local ? process.env.CERT_PR_ASSETS : path.join(root, '.cert/assets');
  if (!dir) throw new Error('exact local PR assets unavailable');
  mkdirSync(dir, { recursive: true });
  const release = local
    ? json(path.join(dir, 'asset-manifest.json'))
    : gh(`repos/${repo}/releases/tags/v${plan.version}`);
  if (local && JSON.stringify(prIdentity(release)) !== JSON.stringify(prIdentity(plan)))
    throw new Error('asset manifest source/run identity mismatch');
  const available = local ? release.files : release.assets;
  const assets =
    process.platform === 'darwin'
      ? available.filter((asset) => asset.name.includes('-darwin-'))
      : available;
  if (!local)
    run(
      'gh',
      [
        'release',
        'download',
        `v${plan.version}`,
        '--repo',
        repo,
        '--dir',
        dir,
        '--clobber',
        ...(process.platform === 'darwin' ? ['--pattern', '*-darwin-*'] : []),
      ],
      { timeout: 300000 }
    );
  return { local, dir, release, assets };
}

await evidence(process.platform === 'darwin' ? 'assets-macos' : 'assets-linux', async (plan) => {
  const { local, dir, release, assets } = assetSource(plan);
  const files = [];
  for (const asset of assets) {
    if (!/^[a-zA-Z0-9.-]+$/.test(asset.name)) throw new Error('invalid asset name');
    const file = path.join(dir, asset.name);
    const sha256 = digest(file);
    const published = asset.digest;
    const check = {
      file: asset.name,
      sha256,
      published,
      status: published === `sha256:${sha256}` ? 'PASS' : published ? 'FAIL' : 'INCOMPLETE',
    };
    if (process.platform === 'darwin') {
      check.signature = verifySignature(file, asset.name);
      if (check.signature.status !== 'PASS' && check.status !== 'FAIL')
        check.status = check.signature.status;
    }
    files.push(check);
  }
  let checksums;
  if (process.platform !== 'darwin') {
    const output = capture(process.execPath, [
      path.join(root, 'scripts/cert/asset-check.mjs'),
      '--directory',
      dir,
      ...checksumMode(plan),
    ]);
    checksums = JSON.parse(output.stdout);
  }
  const missing = process.platform === 'darwin' && files.length !== 4;
  return {
    status:
      files.some((file) => file.status === 'FAIL') || checksums?.status === 'FAIL'
        ? 'FAIL'
        : missing ||
            files.some((file) => file.status === 'INCOMPLETE') ||
            checksums?.status === 'INCOMPLETE'
          ? 'INCOMPLETE'
          : 'PASS',
    files,
    checksums,
    sourceManifest: local ? release : undefined,
    release: release.html_url,
  };
});
