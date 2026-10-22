import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { fixtureServer } from '@ai-ecoverse/slicc-browsers/fixture';
import { run, summary } from '@ai-ecoverse/slicc-browsers/guard-runner';
import { launchBrowser, rawSocket } from './browser.mjs';
import { guarded, observer } from './guard-client.mjs';
import { guardDiagnostics } from './guard-diagnostics.mjs';
import { browserArgs, browserStartup, origins } from './guard-fixture.mjs';

export const published = JSON.parse(
  await readFile(
    fileURLToPath(import.meta.resolve('@ai-ecoverse/slicc-browsers/guard-vectors.json')),
    'utf8'
  )
);

export const partitions = {
  a: ['seven', 'ordinary', 'chrome-newtab', 'chrome-passwords'],
  b: ['vault', 'chrome-new-tab-page', 'chrome-settings'],
  c: ['vault-x', 'own', 'chrome-search'],
  d: ['apex', 'chrome-extension-page', 'file-root', 'network-error'],
};

export const rowKey = (row) => JSON.stringify([row.id, row.target, row.variant]);

export function rowsFor(targets) {
  return published.vectors.flatMap((vector) =>
    vector.targets
      .filter((target) => targets.includes(target))
      .flatMap((target) =>
        Object.keys(published.variants).map((variant) => ({ id: vector.id, target, variant }))
      )
  );
}

export const mandatory = (id) =>
  id.startsWith('C2.redirect-') || /^C4\.iframe-preattach\..*\.shared$/.test(id);

export function canonicalSkip(vector, target) {
  if (vector.skip?.target === target) return vector.skip.reason;
  if (vector.observable?.[target] === false) return 'not-applicable:unobservable-target';
  if (JSON.stringify(vector).includes('{ordinary-same-site}'))
    return 'not-applicable:no-ordinary-same-site-origin';
  return null;
}

export function verdictProblems(results) {
  const problems = [];
  for (const row of results) {
    const vector = published.vectors.find((vector) => vector.id === row.id);
    const legs = [row.control, row.guarded, row.live];
    const where = `${row.id} ${row.target} ${row.variant}`;
    if (legs.every((leg) => leg === 'pass')) continue;
    if (mandatory(row.id)) {
      problems.push(
        `${where} must pass and is never skipped: ${legs.join('/')} ${row.reason ?? ''}`
      );
      continue;
    }
    if (vector?.kind === 'text' && legs.every((leg) => leg === 'not-run')) continue;
    const reason = canonicalSkip(vector, row.target);
    const inactive =
      vector?.skip?.target === row.target || vector?.observable?.[row.target] === false;
    const declared = row.reason === reason || (inactive && row.reason === 'not-applicable');
    if (legs.every((leg) => leg === 'skip') && reason && declared) continue;
    problems.push(
      `${where} is ${legs.join('/')}${row.reason ? ` (${row.reason})` : ''}, not PASS${reason ? ` or the declared skip "${reason}"` : ''}`
    );
  }
  return problems;
}

const extensionPath = fileURLToPath(
  new URL('./harness/extension', import.meta.resolve('@ai-ecoverse/slicc-shared-web/package.json'))
);

async function guardBrowser(startup, { privileged, extension, target, slot }) {
  const browser = await startup.start(
    [
      ...browserArgs({ privileged: privileged || extension }),
      ...(extension ? ['--enable-unsafe-extension-debugging'] : []),
    ],
    { target, slot }
  );
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

export async function runPartition(t, id) {
  const selectedTargets = partitions[id];
  const set = {
    ...published,
    vectors: published.vectors
      .map((vector) => ({
        ...vector,
        targets: vector.targets.filter((target) => selectedTargets.includes(target)),
      }))
      .filter((vector) => vector.targets.length),
  };
  const started = Date.now();
  execFileSync(
    'cargo',
    ['build', '--locked', '-p', 'slicc-cdp-client', '--example', 'guard_bridge'],
    {
      stdio: 'inherit',
    }
  );
  if (process.env.SLICC_GUARD_TRANSPORT === 'wasi')
    execFileSync(
      'cargo',
      [
        'build',
        '--locked',
        '-p',
        'slicc-cdp-client',
        '--example',
        'guard_bridge',
        '--target',
        'wasm32-wasip1',
      ],
      { stdio: 'inherit' }
    );
  const parallel = Number(process.env.SLICC_GUARD_PARALLEL ?? 9);
  const diagnostics = [];
  const tracedClients = new WeakMap();
  const activeTraceEvents = new Set();
  const startup = browserStartup(launchBrowser, (retry) => {
    process.stdout.write(`chrome-startup-retry ${JSON.stringify(retry)}\n`);
  });
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
          ) {
            startup.markRowStarted();
            return {};
          }
          if (!browsers.has(slot))
            browsers.set(
              slot,
              guardBrowser(startup, {
                privileged: vector.id.startsWith('C8.'),
                extension: selected.vectors.some((row) => row.id.startsWith('C9.')),
                target,
                slot,
              })
            );
          const browser = await browsers.get(slot);
          const trace = ['C5.document', 'C5.continue-request'].includes(vector.id)
            ? await guardDiagnostics(browser.wsUrl, { id: vector.id, target, variant, slot })
            : null;
          let client;
          try {
            client = await guarded(trace?.url ?? browser.wsUrl, targets.client, {
              variant,
              detachDelay,
              onMessage: trace?.onMessage,
            });
          } catch (error) {
            await trace?.close();
            throw error;
          }
          if (trace) {
            trace.wrap(client);
            tracedClients.set(client.events, trace);
            activeTraceEvents.add(client.events);
            diagnostics.push(trace.record);
          }
          let unguarded;
          try {
            unguarded = await observer(browser.wsUrl);
          } catch (error) {
            await client.close();
            await trace?.close();
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
          startup.markRowStarted();
          return {
            client,
            unguarded,
            close: async () => {
              if (diagnostic)
                diagnostic.afterCleanup = diagnostic.references.map((details) =>
                  structuredClone(details)
                );
              try {
                await client.close();
                await unguarded.close();
              } finally {
                activeTraceEvents.delete(client.events);
                await trace?.close();
              }
            },
          };
        },
        debug: (details) => {
          if (!details.events?.length) return;
          for (const events of activeTraceEvents) {
            if (events.includes(details.events[0])) tracedClients.get(events)?.snapshot(details);
          }
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
  const settled = await Promise.allSettled(targetNames.map(executeTarget));
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
  report.partition = id;
  report.startupRetries = startup.retries;
  report.selection = { targets: selectedTargets };
  report.transport =
    process.env.SLICC_GUARD_TRANSPORT === 'wasi' ? 'wasi-kernel-cdp' : 'native-websocket';
  report.results = report.results.map((row) => {
    const vector = published.vectors.find((vector) => vector.id === row.id);
    const reason = row.control === 'skip' && vector && canonicalSkip(vector, row.target);
    return reason && row.reason === 'not-applicable' ? { ...row, reason } : row;
  });
  const problems = verdictProblems(report.results);
  const ms = Date.now() - started;
  const output = `${report.line}\nPartition ${id}: ${selectedTargets.join(', ')}\nRuntime: ${(ms / 1000).toFixed(1)}s\nStrict verdicts: ${problems.length ? `${problems.length} problems` : 'every row PASS or its declared skip'}\n\n${table(report.results)}\n`;
  const file =
    process.env.SLICC_GUARD_REPORT ??
    path.join(fileURLToPath(new URL('../../artifacts/', import.meta.url)), `guard-${id}.md`);
  await mkdir(path.dirname(file), { recursive: true });
  await writeFile(file, output);
  await writeFile(
    `${file}.json`,
    JSON.stringify(
      {
        ...report,
        ms,
        parallel,
        problems,
        diagnostics: diagnostics.map(({ references, ...record }) => record),
      },
      null,
      2
    )
  );
  process.stdout.write(output);
  t.diagnostic(`report: ${file}`);
  assert.deepEqual(
    report.results.map(rowKey).sort(),
    rowsFor(selectedTargets).map(rowKey).sort(),
    `partition ${id} must record every published row of its targets, once`
  );
  assert.deepEqual(report.missing, [], 'all published rows must be present');
  assert.equal(report.observerSkipped, 0);
  assert.deepEqual(problems, [], 'every row is PASS or its declared not-applicable skip');
  assert.equal(report.pass, true, report.line);
}
