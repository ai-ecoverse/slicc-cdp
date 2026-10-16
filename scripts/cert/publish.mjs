import { writeFileSync } from 'node:fs';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { json, repo, run } from './common.mjs';

const { values } = parseArgs({ options: { record: { type: 'string' } } });
if (process.env.GITHUB_REF !== 'refs/heads/main')
  throw new Error('release evidence writes are main-only');
const record = json(values.record);
const state = record.certified ? 'success' : record.status === 'INCOMPLETE' ? 'error' : 'failure';
const url = `https://github.com/${repo}/actions/runs/${record.run}`;
run('gh', [
  'api',
  `repos/${repo}/statuses/${record.commit}`,
  '-f',
  `state=${state}`,
  '-f',
  'context=cert-review',
  '-f',
  `target_url=${url}`,
  '-f',
  `description=${record.line.slice(0, 140)}`,
]);
const release = JSON.parse(
  run('gh', ['release', 'view', `v${record.version}`, '--repo', repo, '--json', 'body'])
);
const notes = path.join(path.dirname(values.record), 'release-notes.md');
const body = release.body.replace(/\n*## Certification evidence\n[\s\S]*$/, '').trimEnd();
writeFileSync(
  notes,
  `${body}\n\n## Certification evidence\n\n${record.line}\n\n[CI evidence](${url})\n`
);
run('gh', ['release', 'edit', `v${record.version}`, '--repo', repo, '--notes-file', notes]);
console.log(record.line);
