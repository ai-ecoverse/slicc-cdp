import { spawn } from 'node:child_process';
import { appendFileSync, mkdirSync, realpathSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { testArguments } from '../../test/integration/test-arguments.mjs';
import { cleanEnv, save } from './common.mjs';

export async function suite({ cwd, files, out, env = {}, retry = true }) {
  cwd = realpathSync(cwd);
  const behaviour = (row) =>
    ['test:pass', 'test:fail'].includes(row.type) &&
    !(row.file && row.name && path.resolve(cwd, row.name) === path.resolve(row.file));
  const completed = (worker) =>
    worker.rows.some(behaviour) ||
    worker.rows.some(
      (row) =>
        row.type === 'test:summary' &&
        row.file &&
        path.resolve(row.file) === path.resolve(cwd, worker.file) &&
        row.counts?.tests > 0 &&
        row.counts.cancelled === 0 &&
        row.counts.skipped === 0 &&
        row.counts.todo === 0
    );
  mkdirSync(path.dirname(out), { recursive: true });
  const attempts = [];
  const execute = async (selected) => {
    const index = attempts.length + 1;
    const log = `${out}.${index}.jsonl`;
    const stderr = `${out}.${index}.log`;
    writeFileSync(log, '');
    writeFileSync(stderr, '');
    const workers = [];
    const pending = [...selected];
    const consume = async () => {
      while (pending.length) {
        const file = pending.shift();
        const child = spawn(
          process.execPath,
          [
            ...testArguments,
            '--test-concurrency=1',
            `--test-reporter=${path.join(import.meta.dirname, 'reporter.mjs')}`,
            file,
          ],
          { cwd, env: { ...cleanEnv(), ...env }, detached: process.platform !== 'win32' }
        );
        let buffered = '';
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
              appendFileSync(stderr, `${file}: invalid reporter line\n`);
            }
          }
        });
        child.stderr.on('data', (chunk) => appendFileSync(stderr, `${file}: ${chunk}`));
        const timer = setTimeout(() => {
          if (process.platform === 'win32') child.kill('SIGKILL');
          else {
            try {
              process.kill(-child.pid, 'SIGKILL');
            } catch {
              child.kill('SIGKILL');
            }
          }
        }, 650000);
        const outcome = await new Promise((resolve) => {
          child.once('error', (error) => resolve({ exit: null, error: error.message }));
          child.once('close', (exit, signal) => resolve({ exit, signal }));
        });
        clearTimeout(timer);
        workers.push({ file, ...outcome, rows });
      }
    };
    await Promise.all([consume(), consume()]);
    const lost = workers.some((worker) => worker.exit === null || worker.signal);
    const result = {
      files: selected,
      exit: lost ? null : workers.some((worker) => worker.exit !== 0) ? 1 : 0,
      rows: workers.flatMap((worker) => worker.rows),
      workers,
    };
    attempts.push(result);
    save(`${out}.${index}.json`, result);
    return result;
  };
  const first = await execute(files);
  const repeated = first.workers
    .filter(
      (worker) =>
        !completed(worker) ||
        worker.exit !== 0 ||
        worker.rows.some((row) => row.skip || row.todo || row.type === 'test:fail')
    )
    .map((worker) => worker.file);
  if (retry && repeated.length) await execute(repeated);
  const latest = attempts.at(-1);
  const workers =
    attempts.length === 1
      ? first.workers
      : [...first.workers.filter((worker) => !repeated.includes(worker.file)), ...latest.workers];
  const rows = workers.flatMap((worker) => worker.rows);
  const missing = workers.filter((worker) => !completed(worker)).map((worker) => worker.file);
  const incomplete =
    missing.length ||
    workers.some((worker) => worker.exit === null || worker.signal) ||
    rows.some(
      (row) =>
        row.skip ||
        row.todo ||
        ['cancelledByParent', 'testTimeoutFailure'].includes(row.failureType)
    );
  const result = {
    status: incomplete
      ? 'INCOMPLETE'
      : workers.some((worker) => worker.exit !== 0) || rows.some((row) => row.type === 'test:fail')
        ? 'FAIL'
        : 'PASS',
    files,
    missing,
    rows,
    attempts,
  };
  save(`${out}.json`, result);
  return result;
}
