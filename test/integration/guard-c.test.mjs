import test from 'node:test';
import { partitions, runPartition } from './guard-set.mjs';

test(
  `published guard set, partition c: ${partitions.c.join(', ')}`,
  {
    timeout: Number(process.env.SLICC_GUARD_TIMEOUT ?? 900000),
  },
  (t) => runPartition(t, 'c')
);
