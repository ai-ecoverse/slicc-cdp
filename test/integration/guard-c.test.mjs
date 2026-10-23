import test from 'node:test';
import { chromeTestArguments } from './chrome-policy.mjs';
import { partitions, runPartition } from './guard-set.mjs';

test(
  `published guard set, partition c: ${partitions.c.join(', ')}`,
  ...chromeTestArguments({ timeout: Number(process.env.SLICC_GUARD_TIMEOUT ?? 900000) }, (t) =>
    runPartition(t, 'c')
  )
);
