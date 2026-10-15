import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { fixtureServer } from '@ai-ecoverse/slicc-browsers/fixture';
import { run } from '@ai-ecoverse/slicc-browsers/guard-runner';
import { launchBrowser } from './browser.mjs';
import { guarded, observer } from './guard-client.mjs';
import { browserArgs, origins } from './guard-fixture.mjs';

const set = JSON.parse(
  await readFile(
    fileURLToPath(import.meta.resolve('@ai-ecoverse/slicc-browsers/guard-vectors.json')),
    'utf8'
  )
);
const parityIds = new Set([
  'C5.fetch',
  'C5.xhr',
  'C5.image',
  'C5.auth',
  'C5.guessed-id',
  'C1.href',
  'C1.replace',
  'C1.form',
  'C2.302',
  'C2.307',
  'C2.twohop',
  'C3.popup',
  'C4.iframe',
  'C5.fulfill',
  'C5.continue-url',
  'C5.paused',
  'E1.onload',
  'E2.binding',
  'E3.bypass-csp',
  'E5.console',
  'B2.headers',
  'C6.getCookies',
  'C6.getAllCookies',
  'C6.setCookie',
  'C6.deleteCookies',
  'C6.storage-getCookies',
  'C6.clearDataForOrigin',
]);

function table(results) {
  return [
    '| Row | Target | Variant | Control | Guarded | Live | Reason |',
    '| --- | --- | --- | --- | --- | --- | --- |',
    ...results.map(
      (row) =>
        `| ${row.id} | ${row.target} | ${row.variant} | ${row.control} | ${row.guarded} | ${row.live} | ${(row.reason ?? row.why ?? '').replaceAll('|', '\\|')} |`
    ),
  ].join('\n');
}

function codedSkips(results) {
  return results.map((row) => {
    if (row.control !== 'skip') return row;
    const vector = set.vectors.find((v) => v.id === row.id);
    const missing = vector.requires?.find((cap) => cap === 'autoAttach');
    const reason =
      vector.skip?.target === row.target
        ? vector.skip.reason
        : missing
          ? `not-supported:${missing}`
          : vector.observable?.[row.target] === false
            ? 'not-applicable:unobservable-target'
            : row.reason;
    return { ...row, reason };
  });
}

test('published guard set through the Rust guarded client', { timeout: 600000 }, async (t) => {
  const started = Date.now();
  execFileSync('cargo', ['build', '-p', 'slicc-cdp-client', '--example', 'guard_bridge'], {
    stdio: 'inherit',
  });
  const fixture = await fixtureServer();
  const targets = origins(fixture);
  const browsers = new Map();
  try {
    const execution = await run(set, {
      fixture,
      targets,
      capabilities: {
        scope: 'slicc-cdp',
        autoAttach: false,
        browserSession: true,
        browserContexts: true,
      },
      variant: Object.keys(set.variants),
      parallel: 16,
      clientFactory: async ({ variant, detachDelay, vector, target }) => {
        if (
          vector.kind === 'text' ||
          vector.requires?.includes('autoAttach') ||
          vector.observable?.[target] === false ||
          vector.skip?.target === target
        )
          return {};
        const pool = 0;
        if (!browsers.has(pool)) browsers.set(pool, launchBrowser(browserArgs()));
        const browser = await browsers.get(pool);
        const client = await guarded(browser.wsUrl, targets.client, { variant, detachDelay });
        const unguarded = await observer(browser.wsUrl);
        return {
          client,
          unguarded,
          close: async () => {
            await client.close();
            await unguarded.close();
          },
        };
      },
      log: (row) => {
        if ([row.control, row.guarded, row.live].includes('fail'))
          process.stdout.write(`${JSON.stringify(row)}\n`);
      },
    });
    const report = execution.summary({
      guard: 'slicc-cdp',
      sha: execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(),
    });
    report.results = codedSkips(report.results);
    report.skipped = report.results
      .filter((row) => row.control === 'skip')
      .map(({ id, target, variant, reason }) => ({ id, target, variant, reason }));
    const parity = report.results.filter(
      (row) => parityIds.has(row.id) && ['seven', 'vault'].includes(row.target)
    );
    const passing = parity.filter((row) =>
      [row.control, row.guarded].every((status) => status === 'pass')
    ).length;
    const skipped = parity.filter((row) => row.control === 'skip').length;
    const ms = Date.now() - started;
    const output = `${report.line}\nRuntime: ${(ms / 1000).toFixed(1)}s\nParity: ${passing}/${parity.length - skipped} executed; ${skipped} coded skips (${parity.length} recorded rows; before: 104/108; expected after C5.auth: 108/108)\n\n${table(report.results)}\n`;
    const file =
      process.env.SLICC_GUARD_REPORT ?? path.join(tmpdir(), 'slicc-cdp-guard-runner-report.md');
    await writeFile(file, output);
    await writeFile(
      `${file}.json`,
      JSON.stringify({ ...report, ms, parity: { passing, skipped, total: parity.length } }, null, 2)
    );
    process.stdout.write(output);
    t.diagnostic(`report: ${file}`);
    assert.deepEqual(report.missing, [], 'all published rows must be present');
    assert.equal(report.observerSkipped, 0);
    assert.equal(report.pass, true, report.line);
    assert.equal(parity.length, 108, 'recorded parity subset');
    assert.equal(passing + skipped, 108);
  } finally {
    await Promise.all([...browsers.values()].map(async (browser) => (await browser).close()));
    await fixture.close();
  }
});
