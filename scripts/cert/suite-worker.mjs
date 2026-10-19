import { spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { appendFileSync } from 'node:fs';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import { testArguments } from '../../test/integration/test-arguments.mjs';
import { cleanEnv } from './common.mjs';
import { suitePolicy } from './suite-policy.mjs';
import { captureGuardReport } from './suite-report.mjs';

export const guardFile = 'test/integration/guard-runner.test.mjs';

export async function executeWorker({
  cwd,
  file,
  env,
  deadline,
  index,
  out,
  log,
  stderr,
  workerTimeoutMs,
}) {
  const remainingMs = Math.max(0, Math.floor(deadline - performance.now()));
  if (!remainingMs) {
    return {
      file,
      exit: null,
      signal: null,
      started: false,
      timedOut: true,
      timeoutReason: 'suite budget exhausted before worker start',
      rows: [],
    };
  }
  const timeoutMs = Math.min(workerTimeoutMs, remainingMs);
  const workerStarted = performance.now();
  const report = `${out}.${index}.${createHash('sha256').update(file).digest('hex').slice(0, 16)}.${randomUUID()}.guard.md`;
  const child = spawn(
    process.execPath,
    [
      ...testArguments,
      `--test-timeout=${suitePolicy.testTimeoutMs}`,
      '--test-concurrency=1',
      `--test-reporter=${path.join(import.meta.dirname, 'reporter.mjs')}`,
      file,
    ],
    {
      cwd,
      env: {
        ...cleanEnv(),
        ...env,
        SLICC_GUARD_TARGET_PARALLEL: String(suitePolicy.guardTargetParallel),
        SLICC_GUARD_PARALLEL: String(suitePolicy.guardSlotParallel),
        SLICC_GUARD_REPORT: report,
      },
      detached: process.platform !== 'win32',
    }
  );
  child.stdout.setEncoding('utf8');
  child.stderr.setEncoding('utf8');
  let buffered = '';
  let reporterComplete = true;
  let timedOut = false;
  const rows = [];
  child.stdout.on('data', (chunk) => {
    buffered += chunk;
    const lines = buffered.split('\n');
    buffered = lines.pop();
    for (const line of lines) {
      try {
        const row = { ...JSON.parse(line), worker: file };
        rows.push(row);
        appendFileSync(log, `${JSON.stringify(row)}\n`);
      } catch {
        reporterComplete = false;
        appendFileSync(stderr, `${file}: invalid reporter line\n`);
      }
    }
  });
  child.stderr.on('data', (chunk) => appendFileSync(stderr, `${file}: ${chunk}`));
  const timer = setTimeout(() => {
    timedOut = true;
    appendFileSync(stderr, `${file}: worker budget exhausted after ${timeoutMs} ms\n`);
    if (process.platform === 'win32') child.kill('SIGKILL');
    else {
      try {
        process.kill(-child.pid, 'SIGKILL');
      } catch {
        child.kill('SIGKILL');
      }
    }
  }, timeoutMs);
  const outcome = await new Promise((resolve) => {
    child.once('error', (error) => resolve({ exit: null, error: error.message }));
    child.once('close', (exit, signal) => resolve({ exit, signal }));
  });
  clearTimeout(timer);
  if (buffered) {
    reporterComplete = false;
    appendFileSync(stderr, `${file}: incomplete reporter line\n`);
  }
  return {
    file,
    ...outcome,
    started: true,
    elapsedMs: Math.round(performance.now() - workerStarted),
    timeoutMs,
    timedOut,
    reporterComplete,
    timeoutReason: timedOut
      ? timeoutMs < workerTimeoutMs
        ? 'suite budget exhausted'
        : 'worker budget exhausted'
      : null,
    rows,
    ...(file === guardFile ? { guardReport: captureGuardReport(`${report}.json`) } : {}),
  };
}
