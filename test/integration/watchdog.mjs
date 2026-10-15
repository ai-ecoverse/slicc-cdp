import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { after, afterEach, beforeEach } from 'node:test';

const rowMs = Number(process.env.SLICC_ROW_TIMEOUT_MS || 605000);
const exitMs = Number(process.env.SLICC_EXIT_TIMEOUT_MS || 30000);
const file = path.basename(process.argv[1] || 'test');
const started = Date.now();
const rows = new Map();

export function progress(text) {
  const at = ((Date.now() - started) / 1000).toFixed(1);
  process.stderr.write(`[${file} +${at}s] ${text}\n`);
}

export function stuck(what) {
  progress(`hung: ${what}`);
  progress(`active resources: ${process.getActiveResourcesInfo().join(', ')}`);
  progress(`active handles: ${handles()}`);
  progress(`processes:\n${processes()}`);
  process.exit(1);
}

export async function deadline(what, ms, work) {
  const timer = setTimeout(() => stuck(`${what} after ${ms}ms`), ms);
  try {
    return await work;
  } finally {
    clearTimeout(timer);
  }
}

export function killTree(child) {
  try {
    if (process.platform === 'win32') child.kill('SIGKILL');
    else process.kill(-child.pid, 'SIGKILL');
  } catch {
    child.kill('SIGKILL');
  }
  child.stdout?.destroy();
  child.stderr?.destroy();
}

function handles() {
  return process
    ._getActiveHandles()
    .map((handle) => {
      const name = handle?.constructor?.name || typeof handle;
      if (handle?.pid) return `${name} ${handle.pid} ${handle.spawnargs?.join(' ').slice(0, 200)}`;
      if (handle?.remotePort) return `${name} ${handle.remoteAddress}:${handle.remotePort}`;
      if (handle?.listening) return `${name} listening ${JSON.stringify(handle.address())}`;
      return `${name}${handle?._handle?.fd >= 0 ? ` fd ${handle._handle.fd}` : ''}`;
    })
    .join('; ');
}

function processes() {
  try {
    const rows = execFileSync('ps', ['-A', '-o', 'pid=,ppid=,etime=,command='], {
      encoding: 'utf8',
      timeout: 5000,
    })
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line) => {
        const [pid, ppid] = line.split(/\s+/).map(Number);
        return { pid, ppid, line };
      });
    const mine = new Set([process.pid]);
    for (let grew = true; grew; ) {
      grew = false;
      for (const row of rows) {
        if (!mine.has(row.pid) && mine.has(row.ppid)) {
          mine.add(row.pid);
          grew = true;
        }
      }
    }
    return rows
      .filter(
        (row) =>
          (mine.has(row.pid) && row.pid !== process.pid) ||
          (row.ppid === 1 && /chrom|cdp_client-|playwright-cli|curlwright/i.test(row.line))
      )
      .slice(0, 80)
      .map((row) => row.line.slice(0, 300))
      .join('\n');
  } catch (error) {
    return error instanceof Error ? error.message : String(error);
  }
}

beforeEach((t) => {
  progress(`start ${t.fullName}`);
  const timer = setTimeout(() => stuck(`${t.fullName} after ${rowMs}ms`), rowMs);
  timer.unref();
  rows.set(t.fullName, { timer, since: Date.now() });
});

afterEach((t) => {
  const row = rows.get(t.fullName);
  rows.delete(t.fullName);
  if (row) clearTimeout(row.timer);
  progress(`done ${t.fullName}${row ? ` in ${Date.now() - row.since}ms` : ''}`);
});

after(() => {
  setTimeout(() => stuck(`${file} did not exit ${exitMs}ms after its last test`), exitMs).unref();
});
