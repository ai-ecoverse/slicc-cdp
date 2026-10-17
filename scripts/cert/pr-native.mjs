import { copyFileSync, mkdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { args, digest, json, root, run, save } from './common.mjs';
import { prIdentity } from './pr-common.mjs';
import { integrity, unpackLocal } from './pr-subject.mjs';

const options = args({
  directory: { type: 'string' },
  action: { type: 'string', default: 'collect' },
  signed: { type: 'string' },
});
const plan = json(options.plan);
const platforms = json(new URL('./pr-native.json', import.meta.url));
const selected =
  options.action === 'sign-input'
    ? platforms.filter((item) => item.label.startsWith('darwin-'))
    : platforms;
mkdirSync(options.out, { recursive: true });
const sources = [];
for (const platform of selected) {
  const directory = path.join(options.directory, platform.label);
  const metadata = json(path.join(directory, 'build.json'));
  if (
    JSON.stringify(prIdentity(metadata)) !== JSON.stringify(prIdentity(plan)) ||
    metadata.side !== 'candidate' ||
    metadata.subject.commit !== plan.commit ||
    metadata.subject.version !== plan.version ||
    metadata.target !== platform.target ||
    metadata.label !== platform.label
  )
    throw new Error('native build identity mismatch');
  for (const command of ['playwright-cli', 'curlwright']) {
    const file = `${command}-${platform.label}${platform.label.startsWith('windows-') ? '.exe' : ''}`;
    const row = metadata.files.find((item) => item.file === file);
    if (!row || !/^[a-f0-9]{64}$/.test(row.sha256))
      throw new Error('native file digest unavailable');
    const input = path.join(directory, file);
    if (digest(input) !== row.sha256) throw integrity(`native source digest mismatch: ${file}`);
    if (options.action === 'sign-input') {
      const magic = readFileSync(input).subarray(0, 4).toString('hex');
      if (!['cffaedfe', 'feedfacf', 'cafebabe'].includes(magic))
        throw integrity('signing input is not Mach-O');
    }
    copyFileSync(input, path.join(options.out, file));
  }
  sources.push(metadata);
}
if (options.action === 'sign-input') {
  save(path.join(options.out, 'sign-input.json'), { ...prIdentity(plan), sources });
} else {
  const signed = json(path.join(options.signed, 'signed.json'));
  if (JSON.stringify(prIdentity(signed)) !== JSON.stringify(prIdentity(plan)))
    throw new Error('signer identity mismatch');
  for (const row of signed.files) {
    if (!/^(playwright-cli|curlwright)-darwin-(amd64|arm64)$/.test(row.file))
      throw new Error('unexpected signed file');
    const original = sources
      .flatMap((source) => source.files)
      .find((entry) => entry.file === row.file);
    if (row.input !== original?.sha256) throw integrity('signer consumed another native build');
    if (digest(path.join(options.signed, row.file)) !== row.sha256)
      throw integrity('signed digest mismatch');
    copyFileSync(path.join(options.signed, row.file), path.join(options.out, row.file));
  }
  if (new Set(signed.files.map((row) => row.file)).size !== 4)
    throw new Error('all four signed binaries required');
  const packed = unpackLocal(plan, plan.version, path.join(root, '.cert/asset-pack'));
  for (const file of ['install.sh', 'LICENSE', 'THIRD-PARTY-NOTICES'])
    copyFileSync(path.join(packed.native, file), path.join(options.out, file));
  run(process.execPath, [path.join(root, 'scripts/release-checksums.mjs'), options.out]);
  const inventory = json(new URL('./inventory.json', import.meta.url));
  const files = inventory.map((file) => ({
    name: file,
    digest: `sha256:${digest(path.join(options.out, file))}`,
  }));
  save(path.join(options.out, 'asset-manifest.json'), {
    ...prIdentity(plan),
    sources,
    signed,
    files,
  });
}
