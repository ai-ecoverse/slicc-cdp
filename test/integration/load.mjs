import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import os from 'node:os';
import { monitorEventLoopDelay } from 'node:perf_hooks';

const read = (file) => {
  try {
    return readFileSync(file, 'utf8').trim();
  } catch {
    return null;
  }
};

const number = (text) => (text === null || text === 'max' ? null : Number(text));

function events() {
  const text = read('/sys/fs/cgroup/memory.events');
  if (text === null) return null;
  return Object.fromEntries(
    text.split('\n').map((line) => {
      const [key, value] = line.split(' ');
      return [key, Number(value)];
    })
  );
}

function chromes() {
  try {
    return execFileSync('ps', ['-A', '-o', 'comm='], { encoding: 'utf8' })
      .split('\n')
      .filter((name) => /chrom/i.test(name)).length;
  } catch {
    return null;
  }
}

function cpusAllowed() {
  const list = read('/proc/self/status')?.match(/^Cpus_allowed_list:\s*(.+)$/m)?.[1];
  if (!list) return null;
  return list
    .split(',')
    .map((part) => part.split('-').map(Number))
    .reduce((sum, [from, to = from]) => sum + to - from + 1, 0);
}

export function runner() {
  const quota = read('/sys/fs/cgroup/cpu.max')?.split(' ');
  return {
    cpus: os.availableParallelism(),
    cpusListed: os.cpus().length,
    cpusAllowed: cpusAllowed(),
    cpuQuota: quota && quota[0] !== 'max' ? Number(quota[0]) / Number(quota[1]) : null,
    totalMemory: os.totalmem(),
    memoryMax: number(read('/sys/fs/cgroup/memory.max')),
  };
}

export function sample() {
  return {
    at: Date.now(),
    load: os.loadavg()[0],
    freeMemory: os.freemem(),
    memoryCurrent: number(read('/sys/fs/cgroup/memory.current')),
    memoryEvents: events(),
    chromeProcesses: chromes(),
  };
}

export function watchLoad(everyMs = 2000) {
  const first = sample();
  const samples = [first];
  const lag = monitorEventLoopDelay({ resolution: 20 });
  lag.enable();
  const timer = setInterval(() => samples.push(sample()), everyMs);
  timer.unref();
  return () => {
    clearInterval(timer);
    lag.disable();
    const last = sample();
    samples.push(last);
    const max = (key) =>
      samples.reduce((top, row) => (row[key] === null ? top : Math.max(top ?? 0, row[key])), null);
    const min = (key) =>
      samples.reduce(
        (low, row) => (row[key] === null ? low : Math.min(low ?? Infinity, row[key])),
        null
      );
    const delta = (key) =>
      first.memoryEvents && last.memoryEvents
        ? (last.memoryEvents[key] ?? 0) - (first.memoryEvents[key] ?? 0)
        : null;
    return {
      runner: runner(),
      samples: samples.length,
      maxLoad: max('load'),
      maxChromeProcesses: max('chromeProcesses'),
      maxMemoryCurrent: max('memoryCurrent'),
      minFreeMemory: min('freeMemory'),
      memoryHighEvents: delta('high'),
      memoryMaxEvents: delta('max'),
      oomKills: delta('oom_kill'),
      eventLoopLagMs: {
        p50: lag.percentile(50) / 1e6,
        p99: lag.percentile(99) / 1e6,
        max: lag.max / 1e6,
      },
    };
  };
}

export function describeLoad(load) {
  const gib = (bytes) => (bytes === null ? '?' : `${(bytes / 2 ** 30).toFixed(1)} GiB`);
  const { runner: r } = load;
  const ms = (value) => `${Math.round(value)} ms`;
  return `Load: ${r.cpus} CPUs (${r.cpusListed} listed, ${r.cpusAllowed ?? '?'} allowed${r.cpuQuota ? `, quota ${r.cpuQuota}` : ''}), memory ${gib(r.memoryMax ?? r.totalMemory)}; peak load ${load.maxLoad?.toFixed(1)}, peak ${load.maxChromeProcesses} Chrome processes, peak memory ${gib(load.maxMemoryCurrent)}, least free ${gib(load.minFreeMemory)}, OOM kills ${load.oomKills ?? '?'}, memory.max hits ${load.memoryMaxEvents ?? '?'}; event-loop lag p50 ${ms(load.eventLoopLagMs.p50)}, p99 ${ms(load.eventLoopLagMs.p99)}, max ${ms(load.eventLoopLagMs.max)}`;
}
