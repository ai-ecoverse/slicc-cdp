import { spawn } from 'node:child_process';
import { appendFileSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { cleanEnv, save } from './common.mjs';

export async function suite({ cwd, files, out, env = {}, retry = true }) {
  cwd = realpathSync(cwd);
  const behaviour = (row) =>
    ['test:pass', 'test:fail'].includes(row.type) &&
    !(row.file && row.name && path.resolve(cwd, row.name) === path.resolve(row.file));
  mkdirSync(path.dirname(out), { recursive: true });
  const attempts = [];
  const execute = async (selected) => {
    const index = attempts.length + 1;
    const log = `${out}.${index}.jsonl`;
    const stderr = `${out}.${index}.log`;
    writeFileSync(log, '');
    writeFileSync(stderr, '');
    const child = spawn(
      process.execPath,
      [
        '--test',
        '--test-concurrency=2',
        '--test-timeout=600000',
        `--test-reporter=${path.join(import.meta.dirname, 'reporter.mjs')}`,
        ...selected,
      ],
      { cwd, env: { ...cleanEnv(), ...env }, detached: process.platform !== 'win32' }
    );
    child.stdout.on('data', (chunk) => appendFileSync(log, chunk));
    child.stderr.on('data', (chunk) => appendFileSync(stderr, chunk));
    const timer = setTimeout(() => {
      if (process.platform === 'win32') child.kill('SIGKILL');
      else {
        try {
          process.kill(-child.pid, 'SIGKILL');
        } catch {
          child.kill('SIGKILL');
        }
      }
    }, 1500000);
    const outcome = await new Promise((resolve) => {
      child.once('error', (error) => resolve({ exit: null, error: error.message }));
      child.once('close', (exit, signal) => resolve({ exit, signal }));
    });
    clearTimeout(timer);
    const rows = readFileSync(log, 'utf8')
      .split('\n')
      .filter(Boolean)
      .flatMap((line) => {
        try {
          return [JSON.parse(line)];
        } catch {
          return [];
        }
      });
    const result = { files: selected, ...outcome, rows };
    attempts.push(result);
    save(`${out}.${index}.json`, result);
    return result;
  };
  const first = await execute(files);
  const unfinished = files.filter((file) => {
    const rows = first.rows.filter(
      (row) => row.file && path.resolve(row.file) === path.resolve(cwd, file)
    );
    return (
      !rows.some(behaviour) ||
      rows.some(
        (row) =>
          row.skip ||
          row.todo ||
          row.failureType === 'cancelledByParent' ||
          row.failureType === 'testTimeoutFailure'
      )
    );
  });
  const failed = files.filter((file) =>
    first.rows.some(
      (row) =>
        row.type === 'test:fail' && row.file && path.resolve(row.file) === path.resolve(cwd, file)
    )
  );
  const repeated = [...new Set([...unfinished, ...failed])];
  if (retry && repeated.length) await execute(repeated);
  const latest = attempts.at(-1);
  const rows =
    attempts.length === 1
      ? first.rows
      : [
          ...first.rows.filter(
            (row) =>
              !row.file ||
              !repeated.some((file) => path.resolve(cwd, file) === path.resolve(row.file))
          ),
          ...latest.rows,
        ];
  const missing = files.filter(
    (file) =>
      !rows.some(
        (row) => row.file && path.resolve(row.file) === path.resolve(cwd, file) && behaviour(row)
      )
  );
  const incomplete =
    missing.length ||
    rows.some(
      (row) =>
        row.skip ||
        row.todo ||
        (['cancelledByParent', 'testTimeoutFailure', 'testCodeFailure'].includes(row.failureType) &&
          row.name === row.file)
    );
  const result = {
    status:
      incomplete || latest.exit === null || latest.signal
        ? 'INCOMPLETE'
        : rows.some((row) => row.type === 'test:fail') || latest.exit !== 0
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
