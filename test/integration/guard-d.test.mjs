import test from 'node:test';
import { partitions, runPartition } from './guard-set.mjs';

test(
  `published guard set, partition d: ${partitions.d.join(', ')}`,
  {
    timeout: Number(process.env.SLICC_GUARD_TIMEOUT ?? 900000),
  },
  (t) => runPartition(t, 'd')
);
