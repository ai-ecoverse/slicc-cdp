import { execFileSync } from 'node:child_process';
import { appendFileSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { cliVerdict } from './cli-verdict.mjs';
import { save } from './common.mjs';
import { guardShardPlan } from './guard-shards.mjs';

const verdict = cliVerdict('guard-shard-plan: ');
const { values } = parseArgs({ options: { out: { type: 'string' } } });
const set = JSON.parse(
  readFileSync(
    fileURLToPath(import.meta.resolve('@ai-ecoverse/slicc-browsers/guard-vectors.json')),
    'utf8'
  )
);
const plan = {
  ...guardShardPlan(set),
  head: execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(),
  run: process.env.GITHUB_RUN_ID ?? 'local',
  measurementOnly: true,
};
save(values.out, plan);
if (process.env.GITHUB_OUTPUT)
  appendFileSync(
    process.env.GITHUB_OUTPUT,
    'matrix=' +
      JSON.stringify({
        include: plan.shards.map(({ id, targets, expectedRows }) => ({
          id,
          targets,
          expectedRows,
        })),
      }) +
      '\n'
  );
console.log(
  JSON.stringify({
    targets: plan.targets,
    rows: plan.keys.length,
    shards: plan.shards.map(({ id, targets, expectedRows }) => ({ id, targets, expectedRows })),
  })
);
verdict('PASS', plan.keys.length + ' planned rows across four fixed shards');
