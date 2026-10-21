const unavailable = new Set(['cancelledByParent', 'testTimeoutFailure', 'testAborted']);

export function unavailableRow(row) {
  return Boolean(row?.skip || row?.todo || unavailable.has(row?.failureType));
}

export function behaviourFailure(row) {
  if (row?.type !== 'test:fail' || unavailableRow(row) || row.failureType === 'subtestsFailed')
    return false;
  if (
    row.name &&
    (row.name === row.file || row.name === row.worker || row.file?.endsWith('/' + row.name))
  )
    return false;
  return Boolean(row.name);
}

export function incompleteExecution(execution) {
  if (
    !execution ||
    execution.status === 'INCOMPLETE' ||
    execution.missing?.length ||
    !Array.isArray(execution.rows) ||
    !execution.rows.length ||
    execution.rows.some(unavailableRow)
  )
    return true;
  return (
    execution.files?.includes('test/integration/guard-runner.test.mjs') &&
    execution.guardReport?.status !== 'CAPTURED'
  );
}

export function classifyExecution(
  execution,
  incomplete = incompleteExecution(execution),
  exitFailure = false
) {
  if (actualFailure(execution)) return 'FAIL';
  if (incomplete || exitFailure || execution?.rows?.some((row) => row.type === 'test:fail'))
    return 'INCOMPLETE';
  return 'PASS';
}

export function integrationStatus(positive, proofPositive, negative, proofs) {
  if (
    actualFailure(positive) ||
    actualFailure(proofPositive) ||
    proofs.status === 'FAIL' ||
    [positive, proofPositive, negative].some((execution) => execution.failureKind === 'integrity')
  )
    return 'FAIL';
  if (
    positive.status !== 'PASS' ||
    proofPositive.status !== 'PASS' ||
    proofs.status !== 'PASS' ||
    !['PASS', 'FAIL'].includes(negative.status)
  )
    return 'INCOMPLETE';
  return 'PASS';
}

export function actualFailure(execution) {
  if (!execution) return false;
  if (execution.failureKind === 'integrity' || execution.rows?.some(behaviourFailure)) return true;
  if (
    execution.guardReport?.report?.results?.some(
      (row) => row && [row.control, row.guarded, row.live].includes('fail')
    )
  )
    return true;
  if (
    execution.guardReport?.report?.pass === false &&
    Array.isArray(execution.guardReport.report.results) &&
    execution.guardReport.report.results.length &&
    Array.isArray(execution.guardReport.report.missing) &&
    !execution.guardReport.report.missing.length
  )
    return true;
  if (execution.native && actualFailure(execution.native)) return true;
  if (execution.shards?.some((shard) => actualFailure(shard?.suite))) return true;
  return execution.status === 'FAIL' && !incompleteExecution(execution);
}
