import { appendFileSync } from 'node:fs';
import { parseArgs } from 'node:util';
import { gh, json, repo } from './common.mjs';

const { values } = parseArgs({ options: { record: { type: 'string' }, jobs: { type: 'string' } } });
const record = json(values.record);
if (process.env.UPSTREAM !== 'skipped')
  throw new Error('proof requires a skipped release ancestor');
if (record.run !== process.env.GITHUB_RUN_ID)
  throw new Error('record came from another workflow run');
if (record.certified !== false || record.caller?.['upload-assets'] !== 'skipped')
  throw new Error('skipped caller prerequisite was not held in the record');
const jobs = values.jobs
  ? json(values.jobs).jobs
  : gh(`repos/${repo}/actions/runs/${record.run}/jobs?per_page=100`).jobs;
const required = [
  'plan',
  'rebuild',
  'guard',
  'integration',
  'assets (ubuntu-latest)',
  'assets (macos-latest)',
  'record',
];
for (const name of required) {
  const job = jobs.find((job) => job.name === `cert / ${name}`);
  if (!job || ['skipped', 'cancelled'].includes(job.conclusion) || job.status !== 'completed')
    throw new Error(`workflow_call did not execute required job: ${name}`);
}
const line = `verify-call: PASS workflow_call proved for ${record.version}: skipped release ancestor, all seven evidence jobs executed, record=${record.status}`;
console.log(line);
if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${line}\n`);
