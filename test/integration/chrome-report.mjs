import {
  appendFileSync,
  globSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const directory = fileURLToPath(new URL('.', import.meta.url));
export const CHROME_FILES = new Set(
  [
    'browser-message',
    'catalog-ports',
    'chrome-exit',
    'chromium-kernel-basics',
    'chromium-kernel-guards',
    'chromium',
    'current-document',
    'data-document',
    'dialog',
    'discover-payments',
    'discover-sinks',
    'discover-ucp',
    'discover',
    'error-document-race',
    'eval-function',
    'fetch',
    'guard-a',
    'guard-attach-race',
    'guard-b',
    'guard-c',
    'guard-c2-redirect',
    'guard-d',
    'guard-fail-fast',
    'guard-native',
    'guard-persistent',
    'guard-setup-budget',
    'input',
    'kernel-session',
    'launch',
    'plant',
    'privileged-ui',
    'redirect-navigation',
    'redirect-protected',
    'redirect-userinfo',
    'session',
    'sliccy-browser',
    'snapshot-golden',
    'snapshot-oopif-chrome',
    'storage-chrome',
    'webmcp-polyfill-chrome',
    'webmcp-unavailable',
    'webmcp',
  ].map((name) => path.join(directory, `${name}.test.mjs`))
);

export function chromeShardFiles(env = process.env, argv = process.argv) {
  const files = env.SLICC_TEST_FILES ? env.SLICC_TEST_FILES.split(/\s+/) : argv.slice(1);
  return [
    ...new Set(
      files
        .flatMap((file) => (file.includes('*') ? globSync(file) : [file]))
        .map((file) => path.resolve(file))
        .filter((file) => CHROME_FILES.has(file))
    ),
  ];
}

const counts = {
  file: path.resolve(process.argv[1] || ''),
  started: 0,
  finished: 0,
  failed: 0,
  launched: 0,
};

function record(key) {
  const dir = process.env.SLICC_CHROME_REPORT_DIR;
  if (!dir || !CHROME_FILES.has(counts.file)) return;
  counts[key] += 1;
  writeFileSync(path.join(dir, `${process.pid}.json`), JSON.stringify(counts));
}

export async function trackChromeCase(fn, context) {
  record('started');
  try {
    return await fn(context);
  } catch (error) {
    record('failed');
    throw error;
  } finally {
    record('finished');
  }
}

export function chromeLaunched() {
  record('launched');
}

export function reportChromeShard(dir, files, summaryFile, shard) {
  const rows = files.map((file) => ({ file, started: 0, finished: 0, failed: 0, launched: 0 }));
  for (const name of readdirSync(dir).filter((name) => name.endsWith('.json'))) {
    const saved = JSON.parse(readFileSync(path.join(dir, name), 'utf8'));
    const row = rows.find((row) => row.file === saved.file);
    if (!row) continue;
    for (const key of ['started', 'finished', 'failed', 'launched']) row[key] += saved[key];
  }
  const total = rows.reduce(
    (sum, row) => {
      for (const key of ['started', 'finished', 'failed', 'launched']) sum[key] += row[key];
      return sum;
    },
    { started: 0, finished: 0, failed: 0, launched: 0 }
  );
  let problem;
  if (files.length && !total.started) problem = 'no real-Chrome test cases started';
  else if (rows.some((row) => !row.started)) problem = 'a Chrome file ran no real-Chrome cases';
  else if (rows.some((row) => row.started !== row.finished))
    problem = 'real-Chrome test cases did not all finish';
  else if (files.length && !total.launched) problem = 'no real Chrome launched';
  else if (total.failed) problem = 'real-Chrome test cases failed';
  const status = problem || total.failed ? 'FAIL' : 'PASS';
  const summary = [
    `### Real Chrome, shard ${shard}`,
    '',
    '| File | Started | Finished | Failed | Launched |',
    '| --- | ---: | ---: | ---: | ---: |',
    ...rows.map(
      (row) =>
        `| ${path.basename(row.file)} | ${row.started} | ${row.finished} | ${row.failed} | ${row.launched} |`
    ),
    `| Total | ${total.started} | ${total.finished} | ${total.failed} | ${total.launched} |`,
    '',
    `${status}${problem ? `: ${problem}` : ''}`,
    '',
  ].join('\n');
  if (summaryFile) appendFileSync(summaryFile, `${summary}\n`);
  process.stdout.write(summary);
  return { total, problem };
}

export function prepareChromeReport(env = process.env, argv = process.argv) {
  if (!env.CI || env.NODE_TEST_CONTEXT || env.SLICC_CHROME_REPORT_DIR) return;
  const files = chromeShardFiles(env, argv);
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-chrome-report-'));
  env.SLICC_CHROME_REPORT_DIR = dir;
  process.once('exit', () => {
    try {
      const { problem } = reportChromeShard(
        dir,
        files,
        env.GITHUB_STEP_SUMMARY,
        env.SLICC_TEST_SHARD || 'standalone'
      );
      if (problem) {
        process.stderr.write(`CI Chrome proof failed: ${problem}\n`);
        process.exitCode = 1;
      }
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
}
