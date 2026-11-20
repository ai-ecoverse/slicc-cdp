import test from 'node:test';
import { chromeTestArguments } from './chrome-policy.mjs';
import { partitions, runPartition } from './guard-set.mjs';

process.env.SLICC_GUARD_VECTOR =
  'C3.popup-self-nav,C3.popup-on-redirect-302,C3.popup-on-redirect-chain,C4.static-cross-site,C4.script-cross-site,C4.static-same-site,C4.script-same-site,C10.cookie-plant*';

test(
  `published guard set, partition b: ${partitions.b.join(', ')}`,
  ...chromeTestArguments({ timeout: Number(process.env.SLICC_GUARD_TIMEOUT ?? 900000) }, (t) =>
    runPartition(t, 'b')
  )
);
