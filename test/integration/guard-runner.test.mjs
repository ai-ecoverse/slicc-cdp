import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { fixtureServer } from '@ai-ecoverse/slicc-browsers/fixture';
import { run, summary } from '@ai-ecoverse/slicc-browsers/guard-runner';
import { launchBrowser, rawSocket } from './browser.mjs';
import { guarded, observer } from './guard-client.mjs';
import { browserArgs, origins } from './guard-fixture.mjs';

const published = JSON.parse(
  await readFile(
    fileURLToPath(import.meta.resolve('@ai-ecoverse/slicc-browsers/guard-vectors.json')),
    'utf8'
  )
);
const allTargets = [...new Set(published.vectors.flatMap((vector) => vector.targets))];
const selectedTargets = process.env.SLICC_GUARD_TARGETS
  ? JSON.parse(process.env.SLICC_GUARD_TARGETS)
  : allTargets;
if (
  !Array.isArray(selectedTargets) ||
  !selectedTargets.length ||
  new Set(selectedTargets).size !== selectedTargets.length ||
  selectedTargets.some((target) => !allTargets.includes(target))
)
  throw new Error('guard shard must select unique published targets');
const set = {
  ...published,
  vectors: published.vectors
    .map((vector) => ({
      ...vector,
      targets: vector.targets.filter((target) => selectedTargets.includes(target)),
    }))
    .filter((vector) => vector.targets.length),
};
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

const extensionPath = fileURLToPath(
  new URL('./harness/extension', import.meta.resolve('@ai-ecoverse/slicc-shared-web/package.json'))
);

async function guardBrowser({ privileged, extension }) {
  const browser = await launchBrowser([
    ...browserArgs({ privileged: privileged || extension }),
    ...(extension ? ['--enable-unsafe-extension-debugging'] : []),
  ]);
  if (!extension) return browser;
  let raw;
  try {
    raw = await rawSocket(browser.wsUrl);
    const { id } = await raw.send('Extensions.loadUnpacked', {
      path: extensionPath,
      enableInIncognito: true,
    });
    assert.equal(id, 'ankmbodmgimjemnojmeddfcdlbocolob');
    return browser;
  } catch (error) {
    await browser.close();
    throw error;
  } finally {
    raw?.close();
  }
}

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
    const reason =
      vector.skip?.target === row.target
        ? vector.skip.reason
        : vector.observable?.[row.target] === false
          ? 'not-applicable:unobservable-target'
          : row.reason;
    return { ...row, reason };
  });
}

test('published guard set through the Rust guarded client', { timeout: 900000 }, async (t) => {
  const started = Date.now();
  execFileSync('cargo', ['build', '-p', 'slicc-cdp-client', '--example', 'guard_bridge'], {
    stdio: 'inherit',
  });
  if (process.env.SLICC_GUARD_TRANSPORT === 'wasi')
    execFileSync(
      'cargo',
      ['build', '-p', 'slicc-cdp-client', '--example', 'guard_bridge', '--target', 'wasm32-wasip1'],
      { stdio: 'inherit' }
    );
  const parallel = Number(process.env.SLICC_GUARD_PARALLEL ?? 9);
  const diagnostics = [];
  const executeTarget = async (target) => {
    const fixture = await fixtureServer();
    const targets = origins(fixture);
    const browsers = new Map();
    const selected = {
      ...set,
      vectors: set.vectors
        .filter((vector) => vector.targets.includes(target))
        .map((vector) => ({ ...vector, targets: [target] })),
    };
    try {
      const execution = await run(selected, {
        fixture,
        targets,
        capabilities: {
          scope: 'slicc-cdp',
          autoAttach: true,
          browserSession: true,
          chromeUrlsAttach: true,
          extensionWorld: true,
          browserContexts: true,
        },
        variant: Object.keys(set.variants),
        parallel,
        clientFactory: async ({ slot, variant, detachDelay, vector }) => {
          t.signal.throwIfAborted();
          if (
            vector.kind === 'text' ||
            vector.observable?.[target] === false ||
            vector.skip?.target === target
          )
            return {};
          if (!browsers.has(slot))
            browsers.set(
              slot,
              guardBrowser({
                privileged: vector.id.startsWith('C8.'),
                extension: selected.vectors.some((row) => row.id.startsWith('C9.')),
              })
            );
          const browser = await browsers.get(slot);
          const client = await guarded(browser.wsUrl, targets.client, { variant, detachDelay });
          let unguarded;
          try {
            unguarded = await observer(browser.wsUrl);
          } catch (error) {
            await client.close();
            await browser.close();
            browsers.delete(slot);
            throw error;
          }
          let diagnostic;
          if (vector.id === 'C5.guessed-id.continue-response') {
            diagnostic = {
              id: vector.id,
              target,
              variant,
              slot,
              events: client.events,
              calls: [],
              snapshots: [],
              references: [],
            };
            diagnostics.push(diagnostic);
            const send = client.send;
            client.send = async (method, params = {}, sessionId) => {
              const call = {
                at: Date.now(),
                eventCount: client.events.length,
                method,
                params: structuredClone(params),
                sessionId,
              };
              diagnostic.calls.push(call);
              try {
                const result = await send(method, params, sessionId);
                call.finished = Date.now();
                call.result = structuredClone(result);
                return result;
              } catch (error) {
                call.finished = Date.now();
                call.error = error.cdp ?? { message: error.message };
                throw error;
              }
            };
          }
          return {
            client,
            unguarded,
            close: async () => {
              if (diagnostic)
                diagnostic.afterCleanup = diagnostic.references.map((details) =>
                  structuredClone(details)
                );
              await client.close();
              await unguarded.close();
            },
          };
        },
        debug: (details) => {
          if (!details.events?.length) return;
          const diagnostic = diagnostics.find((record) =>
            record.events.includes(details.events[0])
          );
          if (!diagnostic) return;
          diagnostic.snapshots.push({ at: Date.now(), ...structuredClone(details) });
          diagnostic.references.push(details);
        },
        log: (row) => {
          if (process.env.SLICC_GUARD_PROGRESS)
            process.stdout.write(`row ${row.id} ${row.target} ${row.variant}\n`);
          if ([row.control, row.guarded, row.live].includes('fail'))
            process.stdout.write(`${JSON.stringify(row)}\n`);
        },
      });
      return execution.results;
    } finally {
      await Promise.all([...browsers.values()].map(async (browser) => (await browser).close()));
      await fixture.close();
    }
  };
  const targetNames = [...new Set(set.vectors.flatMap((vector) => vector.targets))];
  const targetParallel = Number(process.env.SLICC_GUARD_TARGET_PARALLEL ?? targetNames.length);
  const settled = [];
  for (let index = 0; index < targetNames.length; index += targetParallel) {
    settled.push(
      ...(await Promise.allSettled(
        targetNames.slice(index, index + targetParallel).map(executeTarget)
      ))
    );
  }
  const rejected = settled.filter((value) => value.status === 'rejected');
  if (rejected.length)
    throw new AggregateError(
      rejected.map((value) => value.reason),
      'target invocations failed'
    );
  const results = settled.flatMap((value) => value.value);
  const report = summary({
    set,
    results,
    guard: 'slicc-cdp',
    sha:
      process.env.SLICC_GUARD_SHA ??
      execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(),
  });
  report.selection = { targets: selectedTargets, allTargets };
  report.transport =
    process.env.SLICC_GUARD_TRANSPORT === 'wasi' ? 'wasi-kernel-cdp' : 'native-websocket';
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
  const output = `${report.line}\nRuntime: ${(ms / 1000).toFixed(1)}s\nParity: ${passing}/${parity.length}; ${skipped} coded skips (before: 104/108)\nParallel: ${targetParallel} of ${settled.length} target invocations, ${parallel} slots each\n\n${table(report.results)}\n`;
  const file =
    process.env.SLICC_GUARD_REPORT ?? path.join(tmpdir(), 'slicc-cdp-guard-runner-report.md');
  await writeFile(file, output);
  await writeFile(
    `${file}.json`,
    JSON.stringify(
      {
        ...report,
        ms,
        parallel,
        parity: { passing, skipped, total: parity.length },
        diagnostics: diagnostics.map(({ references, ...record }) => record),
      },
      null,
      2
    )
  );
  process.stdout.write(output);
  t.diagnostic(`report: ${file}`);
  assert.deepEqual(report.missing, [], 'all published rows must be present');
  assert.equal(report.observerSkipped, 0);
  const expectedParity = set.vectors
    .filter((vector) => parityIds.has(vector.id))
    .reduce(
      (total, vector) =>
        total +
        vector.targets.filter((target) => ['seven', 'vault'].includes(target)).length *
          Object.keys(set.variants).length,
      0
    );
  assert.equal(parity.length, expectedParity, 'recorded parity subset');
  if (!process.env.SLICC_GUARD_TARGETS) assert.equal(expectedParity, 108);
  assert.equal(skipped, 0);
  assert.equal(passing, expectedParity);
  const c8 = report.results.filter((row) => row.id.startsWith('C8.'));
  if (set.vectors.some((vector) => vector.id.startsWith('C8.')))
    assert(c8.length > 0, 'published privileged UI rows ran');
  assert(
    c8.every((row) => [row.control, row.guarded, row.live].every((status) => status === 'pass'))
  );
  const c9 = report.results.filter((row) => row.id.startsWith('C9.'));
  for (const vector of set.vectors.filter((row) => row.id.startsWith('C9.'))) {
    const rows = c9.filter((row) => row.id === vector.id);
    assert.equal(rows.length, vector.targets.length * Object.keys(set.variants).length);
    assert(
      rows.every((row) =>
        [row.control, row.guarded, row.live].every((status) => status === 'pass')
      ),
      `${vector.id} must pass without skips: ${JSON.stringify(rows)}`
    );
  }
  assert.equal(report.pass, true, report.line);
});
