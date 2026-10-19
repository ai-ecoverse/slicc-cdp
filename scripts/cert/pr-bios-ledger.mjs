import { isDeepStrictEqual } from 'node:util';

const sha = (value) => /^[a-f0-9]{64}$/.test(value ?? '');
const oid = (value) => /^[a-f0-9]{40}$/.test(value ?? '');
const positive = (value) => /^(?:[1-9]\d*)$/.test(String(value ?? ''));
const outcomes = (value) =>
  Array.isArray(value) &&
  value.length === 3 &&
  value.every((item) => ['pass', 'fail'].includes(item));
const rate = (value) => `${value.filter((item) => item === 'fail').length}/3`;

export function biosAdvisoryLedger(plan, pin, advisories, executions) {
  if (!advisories.length) return [];
  if (
    !/^[\w.-]+\/[\w.-]+$/.test(plan.repository ?? '') ||
    !positive(plan.run) ||
    !positive(plan.attempt) ||
    !positive(plan.pr) ||
    !oid(plan.head) ||
    !oid(plan.commit) ||
    !oid(pin.commit)
  )
    throw new Error('bios ledger subject identity incomplete');
  return advisories.map((row) => {
    if (
      row.classification !== 'base-only-flaky' ||
      !pin.shards.includes(row.shard) ||
      typeof row.key !== 'string' ||
      !row.key ||
      !outcomes(row.base?.outcomes) ||
      !outcomes(row.candidate?.outcomes) ||
      rate(row.base.outcomes) !== row.base.rate ||
      rate(row.candidate.outcomes) !== row.candidate.rate ||
      !['1/3', '2/3'].includes(row.base.rate) ||
      row.candidate.rate !== '0/3'
    )
      throw new Error('bios ledger actual advisory outcomes incomplete');
    const artifacts = Object.fromEntries(
      ['base', 'candidate'].map((side) => {
        const artifact = executions?.[side]?.[row.shard]?.artifact;
        const name = `${side}-canary-shard-canary-${row.shard}`;
        const url = `https://github.com/${plan.repository}/actions/runs/${plan.run}/artifacts/${artifact?.id}`;
        if (
          !positive(artifact?.id) ||
          artifact.name !== name ||
          artifact.url !== url ||
          !sha(artifact.digest) ||
          artifact.zipSha256 !== artifact.digest ||
          !sha(artifact.documentSha256) ||
          !sha(artifact.rowsSha256)
        )
          throw new Error(`bios ledger ${side} uploaded artifact identity incomplete`);
        return [side, { ...artifact }];
      })
    );
    if (String(artifacts.base.id) === String(artifacts.candidate.id))
      throw new Error('bios ledger artifacts are not distinct');
    return {
      schema: 1,
      repository: plan.repository,
      run: String(plan.run),
      attempt: String(plan.attempt),
      pr: plan.pr,
      head: plan.head,
      audit: plan.commit,
      biosPin: pin.commit,
      classification: row.classification,
      shard: row.shard,
      key: row.key,
      test: row.test,
      base: structuredClone(row.base),
      candidate: structuredClone(row.candidate),
      artifacts,
    };
  });
}

export function biosLedgerObservations(validatedRecords) {
  const seen = new Map();
  for (const record of validatedRecords) {
    if (
      !['PASS', 'FAIL', 'INCOMPLETE'].includes(record?.status) ||
      !Array.isArray(record.biosLedger)
    )
      throw new Error('validated ledger record required');
    for (const row of record.biosLedger) {
      if (
        row.repository !== record.repository ||
        String(row.run) !== String(record.run) ||
        String(row.attempt) !== String(record.attempt) ||
        row.pr !== record.pr ||
        row.head !== record.head ||
        row.audit !== record.commit ||
        row.biosPin !== record.bios?.commit
      )
        throw new Error('ledger observation bound to another record');
      const checked = biosAdvisoryLedger(
        record,
        { ...record.bios, shards: [row.shard] },
        [row],
        Object.fromEntries(
          ['base', 'candidate'].map((side) => [
            side,
            { [row.shard]: { artifact: row.artifacts?.[side] } },
          ])
        )
      )[0];
      if (!isDeepStrictEqual(row, checked)) throw new Error('malformed ledger observation');
      const key = JSON.stringify([row.repository, row.run, row.attempt, row.shard, row.key]);
      if (seen.has(key) && !isDeepStrictEqual(seen.get(key), row))
        throw new Error('conflicting duplicate ledger observation');
      seen.set(key, structuredClone(row));
    }
  }
  return [...seen.values()];
}
