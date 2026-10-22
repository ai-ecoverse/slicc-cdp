import { chromeTest as test } from './chrome-policy.mjs';
import { partitions, runPartition } from './guard-set.mjs';

test(
  `published guard set, partition a: ${partitions.a.join(', ')}`,
  {
    timeout: Number(process.env.SLICC_GUARD_TIMEOUT ?? 900000),
  },
  (t) => runPartition(t, 'a')
);
