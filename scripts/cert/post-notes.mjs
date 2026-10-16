import { appendFileSync } from 'node:fs';
import path from 'node:path';
import { args, gh, json, name, repo, run, save } from './common.mjs';

const options = args({ record: { type: 'string' }, dry: { type: 'boolean', default: false } });
if (process.env.GITHUB_REF !== 'refs/heads/main')
  throw new Error('verification notes are main-only');
const record = json(options.record);
if (record.status !== 'PASS' || !record.certified || !/^[a-f0-9]{64}$/.test(record.sha256 ?? ''))
  throw new Error('no complete supply-chain verification to publish');
const release = gh(`repos/${repo}/releases/tags/v${record.version}`);
if (release.draft || release.tag_name !== `v${record.version}`)
  throw new Error('published release required');
const body = release.body ?? '';
const pattern = new RegExp(
  `certified(?:-candidate)? ${name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}@${record.version.replaceAll('.', '\\.')} sha256=([a-f0-9]{64})`,
  'g'
);
const hashes = [...body.matchAll(pattern)].map((match) => match[1]);
if (hashes.some((sha) => sha !== record.sha256))
  throw new Error('supply-chain FAIL: release notes certify different bytes');
const start = '<!-- cert-verify -->';
const end = '<!-- /cert-verify -->';
const begins = body.split(start).length - 1;
const ends = body.split(end).length - 1;
if (begins !== ends || begins > 1 || (begins && body.indexOf(start) > body.indexOf(end)))
  throw new Error('verification notes markers malformed');
if (body.includes(record.line)) console.log('verification notes unchanged');
else {
  const section = `${start}\n## Supply-chain verification\n\n${record.line}\n${end}`;
  const notes = begins
    ? body.replace(/<!-- cert-verify -->[\s\S]*?<!-- \/cert-verify -->/, section)
    : `${body.trimEnd()}\n\n${section}\n`;
  if (!options.dry) {
    const file = path.join(path.dirname(options.record), 'notes-payload.json');
    save(file, { body: notes });
    run('gh', [
      'api',
      `repos/${repo}/releases/${release.id}`,
      '--method',
      'PATCH',
      '--input',
      file,
    ]);
  }
  console.log(options.dry ? 'verification notes dry run' : 'verification notes updated');
}
console.log(record.line);
if (process.env.GITHUB_STEP_SUMMARY)
  appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${record.line}\n`);
