import { performance } from 'node:perf_hooks';

export const suitePolicy = Object.freeze({
  fileWorkers: 2,
  guardTargetParallel: 4,
  guardSlotParallel: 4,
  testTimeoutMs: 900000,
  workerGraceMs: 60000,
  workerTimeoutMs: 960000,
  suiteBudgetMs: 3600000,
  jobBudgetMs: 5400000,
  setupAndUploadReserveMs: 1800000,
  uploadReserveMs: 300000,
});

export function boundedBudget(value, maximum, name) {
  if (!Number.isSafeInteger(value) || value < 10 || value > maximum)
    throw new Error(`${name} must be an integer from 10 to ${maximum}`);
  return value;
}

export function executionBudget({ allowMissing = false } = {}) {
  const marker = process.env.CERT_SUITE_JOB_STARTED_MS;
  if (!marker && process.env.GITHUB_ACTIONS === 'true' && !allowMissing)
    return { status: 'INCOMPLETE', reason: 'cert job start marker unavailable' };
  const now = Date.now();
  const jobStartedMs = marker ? Number(marker) : null;
  if (marker && (!Number.isSafeInteger(jobStartedMs) || jobStartedMs < 1 || jobStartedMs > now))
    return { status: 'INCOMPLETE', reason: 'cert job start marker invalid' };
  const remainingMs = marker
    ? Math.max(0, suitePolicy.jobBudgetMs - (now - jobStartedMs) - suitePolicy.uploadReserveMs)
    : suitePolicy.suiteBudgetMs;
  const availableMs = Math.min(suitePolicy.suiteBudgetMs, remainingMs);
  return {
    status: availableMs > 0 ? 'AVAILABLE' : 'INCOMPLETE',
    reason: availableMs > 0 ? null : 'cert job execution budget exhausted',
    jobStartedMs,
    jobBudgetMs: suitePolicy.jobBudgetMs,
    uploadReserveMs: suitePolicy.uploadReserveMs,
    availableMs,
    deadline: performance.now() + availableMs,
  };
}
