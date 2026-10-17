import { mkdirSync } from 'node:fs';
import path from 'node:path';
import { digest, json, run } from './common.mjs';
import { prIdentity } from './pr-common.mjs';

export function integrity(message) {
  const error = new Error(message);
  error.certStatus = 'FAIL';
  return error;
}

export function unpackLocal(plan, version, directory) {
  const side =
    version === plan.version ? 'candidate' : version === plan.baseline.version ? 'base' : null;
  if (!side || !process.env.CERT_PR_PACKS) throw new Error('exact local PR packs unavailable');
  const packDirectory = path.join(process.env.CERT_PR_PACKS, side);
  const metadata = json(path.join(packDirectory, 'build.json'));
  if (JSON.stringify(prIdentity(metadata)) !== JSON.stringify(prIdentity(plan)))
    throw new Error('pack source/pins/run attempt mismatch');
  const subject =
    side === 'candidate' ? { version: plan.version, commit: plan.commit } : plan.baseline;
  if (
    metadata.side !== side ||
    metadata.subject.commit !== subject.commit ||
    metadata.subject.version !== version ||
    metadata.target !== 'wasm32-wasip1'
  )
    throw new Error('pack subject identity mismatch');
  for (const entry of metadata.files) {
    if (!/^[\w.-]+$/.test(entry.file) || !/^[a-f0-9]{64}$/.test(entry.sha256))
      throw new Error('invalid pack inventory');
    if (digest(path.join(packDirectory, entry.file)) !== entry.sha256)
      throw integrity(`pack digest mismatch: ${side}/${entry.file}`);
  }
  const archives = metadata.files.filter((entry) => entry.package);
  if (archives.length !== 1 || !archives[0].file.endsWith('.tgz'))
    throw new Error('pack tarball inventory missing');
  const tarball = path.join(packDirectory, archives[0].file);
  mkdirSync(directory, { recursive: true });
  const entries = run('tar', ['-tzf', tarball]).split('\n');
  if (
    entries.some((entry) => !entry.startsWith('package/') || entry.split('/').includes('..')) ||
    run('tar', ['-tvzf', tarball])
      .split('\n')
      .some((entry) => !/^[d-]/.test(entry))
  )
    throw integrity('unsafe package archive');
  run('tar', ['-xzf', tarball, '-C', directory]);
  const manifest = json(path.join(directory, 'package/package.json'));
  if (
    manifest.name !== '@ai-ecoverse/slicc-cdp' ||
    manifest.version !== version ||
    manifest.gitHead !== subject.commit
  )
    throw integrity('packed package identity differs from subject');
  for (const command of ['playwright-cli', 'curlwright']) {
    if (!metadata.files.some((entry) => entry.file === `${command}-host`))
      throw new Error('packed host native command missing');
    digest(path.join(directory, 'package/bin', `${command}.wasm`));
  }
  return { tarball, manifest, sha256: digest(tarball), native: packDirectory };
}
