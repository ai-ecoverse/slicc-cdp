import { isDeepStrictEqual } from 'node:util';

const kinds = ['pass', 'skip', 'fail', 'regression', 'both', 'flaky'];
const failedKinds = new Set(['fail', 'regression', 'both']);
const lists = { fail: 'failures', regression: 'regressions', both: 'both', flaky: 'flaky' };
const keyFor = (row) => JSON.stringify([row.shard, row.key]);
const project = (row) => ({
  key: row?.key,
  shard: row?.shard,
  test: row?.test,
  verdict: row?.verdict,
  failed: row?.failed,
});

function reportIdentity(side, result, report, emit) {
  for (const field of [
    'biosRef',
    'ok',
    'run',
    'url',
    'repeat',
    'overrides',
    'counts',
    'missingShards',
    'unexplainedExits',
    'refused',
    'ambiguous',
  ]) {
    if (!Object.hasOwn(report, field)) emit('INCOMPLETE', `${side}: full report ${field} absent`);
    else if (!isDeepStrictEqual(report[field], result?.[field]))
      emit('FAIL', `${side}: full report ${field} differs from the bound summary`);
  }
  const candidates = Array.isArray(report.candidates)
    ? report.candidates.map((row) => ({
        package: row?.package,
        status: row?.status,
        via: row?.via,
      }))
    : undefined;
  if (!isDeepStrictEqual(candidates, result?.candidates))
    emit('FAIL', `${side}: full report candidate identities differ from the bound summary`);
}

function reportSource(side, result, report, emit) {
  if (!report) return result?.tests;
  reportIdentity(side, result, report, emit);
  const suppliedRows = report.tests ?? report.results;
  if (!Array.isArray(suppliedRows)) {
    emit('INCOMPLETE', `${side}: complete full report rows absent`);
    return undefined;
  }
  const rows = suppliedRows.map(project);
  if (
    report.tests !== undefined &&
    Array.isArray(report.results) &&
    !isDeepStrictEqual(rows, report.results.map(project))
  )
    emit('INCOMPLETE', `${side}: full report tests and result row sets differ`);
  if (
    result?.tests !== undefined &&
    !isDeepStrictEqual(Array.isArray(result.tests) ? result.tests.map(project) : undefined, rows)
  )
    emit('INCOMPLETE', `${side}: full report row set differs from producer tests`);
  for (const [kind, list] of Object.entries(lists)) {
    const expected = (Array.isArray(report.results) ? report.results : suppliedRows)
      .filter((row) => row?.verdict === kind)
      .slice(0, 100);
    const actual = result?.[list];
    if (!Array.isArray(actual) || actual.length !== expected.length) {
      emit('INCOMPLETE', `${side}: full report ${list} coverage mismatch`);
      continue;
    }
    for (let index = 0; index < expected.length; index++)
      if (actual[index]?.log !== String(expected[index].log ?? '').slice(0, 300))
        emit('FAIL', `${side}: full report ${list} log differs from the bound summary`);
  }
  return rows;
}

function executed(row, repeat) {
  const match = /^(\d+)\/(\d+)$/.exec(row.failed?.candidate ?? '');
  if (!match || Number(match[2]) !== repeat || repeat !== 3) return false;
  if (row.verdict === 'pass') return Number(match[1]) === 0;
  return failedKinds.has(row.verdict) && Number(match[1]) === repeat;
}

function rowIdentity(side, row, pin, repeat, seen, emit) {
  if (
    !row ||
    typeof row.key !== 'string' ||
    !row.key ||
    typeof row.test !== 'string' ||
    !row.test ||
    !pin.shards.includes(row.shard) ||
    !kinds.includes(row.verdict)
  ) {
    emit('INCOMPLETE', `${side}: stable test identity/verdict absent`);
    return false;
  }
  const key = keyFor(row);
  if (seen.has(key)) emit('INCOMPLETE', `${side}: duplicate test key ${key}`);
  seen.add(key);
  if (!executed(row, repeat))
    emit('INCOMPLETE', `${side}: unexecuted, partial or flaky test ${row.key}`);
  return true;
}

function listMatches(row, expected) {
  return (
    row?.shard === expected.shard &&
    row.test === expected.test &&
    (row.key === undefined || row.key === expected.key) &&
    isDeepStrictEqual(row.failed, expected.failed)
  );
}

function rowCounts(side, rows, result, emit) {
  for (const kind of kinds) {
    const selected = rows.filter((row) => row.verdict === kind);
    if (!Number.isSafeInteger(result?.counts?.[kind]) || result.counts[kind] !== selected.length)
      emit('INCOMPLETE', `${side}: complete ${kind} rows/counts mismatch`);
    const list = lists[kind];
    if (!list) continue;
    const actual = result?.[list];
    const expected = selected.slice(0, 100);
    if (
      !Array.isArray(actual) ||
      actual.length !== expected.length ||
      actual.some((row, index) => !listMatches(row, expected[index]))
    )
      emit('INCOMPLETE', `${side}: complete ${list} rows/list mismatch`);
  }
}

function producerCompletion(side, result, emit) {
  for (const field of ['missingShards', 'unexplainedExits', 'ambiguous'])
    if (!Array.isArray(result?.[field]) || result[field].length)
      emit('INCOMPLETE', `${side}: producer ${field} unfinished`);
  if (!Array.isArray(result?.refused) || result.refused.length)
    emit('INCOMPLETE', `${side}: producer artifact refusal or refusal evidence absent`);
  if (typeof result?.ok !== 'boolean') emit('INCOMPLETE', `${side}: producer verdict absent`);
}

function readRows(side, result, report, pin, emit) {
  producerCompletion(side, result, emit);
  const rows = reportSource(side, result, report, emit);
  if (!Array.isArray(rows) || !rows.length) {
    emit('INCOMPLETE', `${side}: complete producer tests field required`);
    return new Map();
  }
  const seen = new Set();
  const valid = rows.filter((row) => rowIdentity(side, row, pin, result?.repeat, seen, emit));
  rowCounts(side, valid, result, emit);
  for (const shard of pin.shards)
    if (!valid.some((row) => row.shard === shard))
      emit('INCOMPLETE', `${side}: unexecuted planned shard ${shard}`);
  return new Map(valid.map((row) => [keyFor(row), row]));
}

function sameRowSet(base, candidate, emit) {
  if (base.size !== candidate.size) emit('INCOMPLETE', 'base/candidate test row sets differ');
  for (const [key, row] of candidate) {
    const before = base.get(key);
    if (!before || before.shard !== row.shard || before.test !== row.test)
      emit('INCOMPLETE', `base/candidate test identity differs: ${key}`);
  }
}

function producerAcceptance(side, result, rows, shared, candidate, emit, flags) {
  if (result.ok !== false) return;
  const failed = [...rows.values()].filter((row) => failedKinds.has(row.verdict));
  const explained = (row) =>
    shared.has(keyFor(row)) || (side === 'base' && candidate.get(keyFor(row))?.verdict === 'pass');
  if (!failed.length || failed.some((row) => !explained(row)))
    emit(
      'INCOMPLETE',
      `${side}: producer rejection not wholly explained by pre-existing or improved failures`
    );
  else
    flags.push(
      `bios ${side}: producer ok=false explained by ${failed.length} pre-existing or improved failures`
    );
}

export function pairedBiosFindings(results, pin, reports = {}) {
  const failures = [];
  const incomplete = [];
  const flags = [];
  const emit = (status, reason) =>
    (status === 'FAIL' ? failures : incomplete).push(`bios: paired ${reason}`);
  const base = readRows('base', results?.base, reports?.base, pin, emit);
  const candidate = readRows('candidate', results?.candidate, reports?.candidate, pin, emit);
  sameRowSet(base, candidate, emit);
  if (!failures.length && !incomplete.length) {
    const shared = new Set();
    for (const [key, row] of candidate) {
      if (!failedKinds.has(row.verdict)) continue;
      if (base.get(key).verdict === 'pass')
        emit('FAIL', `candidate introduced failure on a base-PASS test: ${key}`);
      else {
        shared.add(key);
        flags.push(`bios pre-existing failure on both PR packs: ${key}`);
      }
    }
    for (const [key, row] of base)
      if (failedKinds.has(row.verdict) && candidate.get(key).verdict === 'pass')
        flags.push(`bios pre-existing base failure improved by candidate: ${key}`);
    for (const side of ['base', 'candidate'])
      producerAcceptance(
        side,
        results[side],
        side === 'base' ? base : candidate,
        shared,
        candidate,
        emit,
        flags
      );
  }
  return { failures: [...new Set(failures)], incomplete: [...new Set(incomplete)], flags };
}
