import test from 'node:test';
import { chromeTestArguments } from './chrome-policy.mjs';
import { partitions, runPartition } from './guard-set.mjs';

process.env.SLICC_GUARD_VECTOR = 'C10.cookie-plant*';

test(
  `published guard set, partition a: ${partitions.a.join(', ')}`,
  ...chromeTestArguments({ timeout: Number(process.env.SLICC_GUARD_TIMEOUT ?? 900000) }, (t) =>
    runPartition(t, 'a')
  )
);
