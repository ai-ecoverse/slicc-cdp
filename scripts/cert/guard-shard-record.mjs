import { existsSync } from 'node:fs';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { json, save } from './common.mjs';
import { recordGuardShards } from './guard-shards.mjs';

const { values } = parseArgs({
  options: { plan: { type: 'string' }, directory: { type: 'string' }, out: { type: 'string' } },
});
const plan = json(values.plan);
const documents = plan.shards.flatMap((shard) => {
  const file = path.join(values.directory, shard.id, 'shard.json');
  return existsSync(file) ? [json(file)] : [];
});
const record = recordGuardShards(plan, documents);
save(values.out, record);
console.log(JSON.stringify(record));
process.exitCode = record.status === 'PASS' ? 0 : 1;
