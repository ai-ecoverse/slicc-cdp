import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { fixtureServer } from '@ai-ecoverse/slicc-browsers/fixture';
import { skipReason } from '@ai-ecoverse/slicc-browsers/guard-driver.mjs';
import { run, summary } from '@ai-ecoverse/slicc-browsers/guard-runner';
import { launchBrowser, rawSocket } from './browser.mjs';
import { requireChrome } from './chrome-policy.mjs';
import { acquire, cleanup, cleanupGrace, deadline, normalizeError } from './cleanup.mjs';
import { recordedClient, requireRedirectRegistration } from './guard-c2-support.mjs';
import { guarded, observer } from './guard-client.mjs';
import { guardDiagnostics } from './guard-diagnostics.mjs';
import { browserArgs, browserStartup, origins } from './guard-fixture.mjs';
import { inboundClients } from './inbound-clients.mjs';
import { recordX1Client, x1Privacy } from './x1-audit.mjs';

const { containsSecret } = await import(
  new URL('./redirect-secret.mjs', import.meta.resolve('@ai-ecoverse/slicc-browsers/package.json'))
    .href
);

export const protectedBfcacheRows = new Set([
  'C8.protected-bfcache-restore',
  'C8.protected-bfcache-restore.history',
]);

export function containsMarkerToken(value, marker) {
  const pattern = new RegExp(
    `(^|[^A-Za-z0-9_-])${marker.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}(?=$|[^A-Za-z0-9_-])`
  );
  const seen = new Set();
  const objects = new Set();
  const visit = (item, depth = 0) => {
    if (item && typeof item === 'object') {
      if (objects.has(item)) return false;
      objects.add(item);
      return Object.values(item).some((field) => visit(field, depth));
    }
    if (typeof item !== 'string') return false;
    let logical = item;
    for (let pass = 0; pass < 8; pass++) {
      const decoded = logical.replace(/%([0-9a-f]{2})|\\u([0-9a-f]{4})/gi, (_, percent, unicode) =>
        String.fromCharCode(parseInt(percent ?? unicode, 16))
      );
      if (decoded === logical) break;
      logical = decoded;
    }
    if (pattern.test(logical)) return true;
    const key = `${depth}:${item}`;
    if (depth >= 8 || seen.has(key)) return false;
    seen.add(key);
    const decoded = [];
    try {
      decoded.push(decodeURIComponent(item));
    } catch {}
    try {
      decoded.push(JSON.parse(item));
    } catch {}
    try {
      decoded.push(JSON.parse(`"${item}"`));
    } catch {}
    for (const [chunk] of item.matchAll(/[A-Za-z0-9+/_-]{12,}={0,2}/g)) {
      const normalized = chunk.replaceAll('-', '+').replaceAll('_', '/').replace(/=+$/, '');
      for (let offset = 0; offset < 4; offset++)
        decoded.push(Buffer.from(normalized.slice(offset), 'base64').toString('utf8'));
    }
    return decoded.some((candidate) => candidate !== item && visit(candidate, depth + 1));
  };
  return visit(value);
}

export function bfcachePrivacy(row, messages) {
  const url = row.evidence?.guarded?.restore?.seed?.protectedURL;
  if (typeof url !== 'string' || !Array.isArray(messages) || !messages.length)
    return { measured: false };
  let parsed;
  try {
    parsed = new URL(url);
  } catch {
    return { measured: false };
  }
  const tag = parsed.searchParams.get('n');
  if (!tag) return { measured: false };
  return {
    measured: true,
    markerExposed: containsMarkerToken(messages, `bfcache-marker-${tag}`),
    ordinaryBMarkerObserved: containsMarkerToken(messages, `bfcache-marker-${tag}-b`),
    upstreamMarkerSubstring: containsSecret(messages, `bfcache-marker-${tag}`),
    markerIdentity: 'whole-token',
    auditDifference:
      !containsMarkerToken(messages, `bfcache-marker-${tag}`) &&
      containsSecret(messages, `bfcache-marker-${tag}`) &&
      containsMarkerToken(messages, `bfcache-marker-${tag}-b`)
        ? 'upstream substring matches the public ordinary-B marker; no whole P marker token delivered'
        : null,
    protectedPathExposed:
      containsSecret(messages, `${parsed.origin}${parsed.pathname}`) ||
      containsSecret(messages, `${parsed.host}${parsed.pathname}`) ||
      containsMarkerToken(messages, `${parsed.pathname}${parsed.search}`),
  };
}

export function bfcacheScreenshotExposed(row) {
  return (
    row.evidence?.guarded?.restore?.probes?.some(
      (probe) =>
        probe.protectedCurrent !== false &&
        probe.method === 'Page.captureScreenshot' &&
        probe.reply?.ok &&
        probe.reply?.result?.data
    ) === true
  );
}

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
  protectedBfcacheRows.has(id) ||
  id === 'X1.after-exit' ||
  id.startsWith('C2.redirect-') ||
  /^C4\.iframe-preattach\..*\.shared$/.test(id);

export function canonicalSkip(vector, target) {
  if (!vector) return null;
  const declared = skipReason(vector, target, 'slicc-cdp');
  if (typeof declared === 'string' && declared.startsWith('not-applicable')) return declared;
  const reason = 'not-applicable:no-ordinary-same-site-origin';
  const row = {
    id: vector.id,
    target,
    variant: Object.keys(published.variants)[0],
    transport: 'cdp',
    control: 'skip',
    guarded: 'skip',
    live: 'skip',
    reason,
  };
  const probe = summary({
    set: {
      ...published,
      variants: { [row.variant]: published.variants[row.variant] },
      vectors: [{ ...vector, targets: [target] }],
    },
    results: [row],
    guard: 'slicc-cdp',
    sha: 'applicability-check',
  });
  return probe.forged.length === 0 ? reason : null;
}

export function validApplicabilitySkip(vector, row) {
  if (!vector || typeof row.reason !== 'string' || !row.reason.startsWith('not-applicable'))
    return false;
  const set = {
    ...published,
    variants: { [row.variant]: published.variants[row.variant] },
    vectors: [{ ...vector, targets: [row.target] }],
  };
  return (
    summary({
      set,
      results: [{ ...row, transport: 'cdp', control: 'skip', guarded: 'skip', live: 'skip' }],
      guard: 'slicc-cdp',
      sha: 'applicability-reason-check',
    }).forged.length === 0
  );
}

export function verdictProblems(results) {
  const problems = [];
  for (const row of results) {
    const vector = published.vectors.find((vector) => vector.id === row.id);
    const legs = [row.control, row.guarded, row.live];
    const where = `${row.id} ${row.target} ${row.variant}`;
    if (
      protectedBfcacheRows.has(row.id) &&
      (row.guardedPrivacy?.measured !== true ||
        row.guardedPrivacy.markerExposed !== false ||
        row.guardedPrivacy.protectedPathExposed !== false ||
        row.observations?.guarded?.markerHidden === false ||
        row.observations?.guarded?.originHidden === false ||
        bfcacheScreenshotExposed(row))
    ) {
      problems.push(`${where} lacks measured non-exposure or discloses protected content`);
      continue;
    }
    if (
      row.id === 'X1.after-exit' &&
      (row.x1Privacy?.measured !== true ||
        row.x1Privacy.markerExposed !== false ||
        row.x1Privacy.protectedExposed !== false ||
        row.x1Privacy.screenshotExposed !== false)
    ) {
      problems.push(`${where} lacks measured X1 non-exposure or discloses protected content`);
      continue;
    }
    if (
      row.id === 'C8.protected-bfcache-restore.history' &&
      (row.guardedForwardingAudit?.id !== 'history-trigger' ||
        row.guardedForwardingAudit?.complete !== true ||
        row.guardedForwardingAudit.forwarded !== false)
    ) {
      problems.push(where + ' lacks terminal upstream non-forwarding proof');
      continue;
    }
    if (legs.every((leg) => leg === 'pass')) continue;
    if (mandatory(row.id)) {
      problems.push(
        `${where} must pass and is never skipped: ${legs.join('/')} ${row.reason ?? ''}`
      );
      continue;
    }
    if (vector?.kind === 'text' && legs.every((leg) => leg === 'not-run')) continue;
    const reason = canonicalSkip(vector, row.target);
    const declared = validApplicabilitySkip(vector, row);
    if (legs.every((leg) => leg === 'skip') && reason && declared) continue;
    problems.push(
      `${where} is ${legs.join('/')}${row.reason ? ` (${row.reason})` : ''}, not PASS${reason ? ` or the declared skip "${reason}"` : ''}`
    );
  }
  return problems;
}

export function assertGuardReport(report, id, head) {
  const targets = partitions[id];
  assert.ok(targets, 'guard partition must be declared');
  const set = {
    ...published,
    vectors: published.vectors
      .map((vector) => ({
        ...vector,
        targets: vector.targets.filter((target) => targets.includes(target)),
      }))
      .filter((vector) => vector.targets.length),
  };
  assert.equal(report.sha, head, 'guard report head binding');
  assert.equal(report.guard, 'slicc-cdp', 'guard scope binding');
  assert.equal(
    report.set,
    `@ai-ecoverse/slicc-browsers@${published.setVersion}`,
    'guard vector binding'
  );
  assert.deepEqual(
    [...report.selection.targets].sort(),
    [...targets].sort(),
    'guard target ownership'
  );
  assert.deepEqual(
    report.results.map(rowKey).sort(),
    rowsFor(targets).map(rowKey).sort(),
    'every assigned row must be recorded exactly once'
  );
  const checked = summary({ set, results: report.results, guard: 'slicc-cdp', sha: head });
  assert.equal(report.pass, checked.pass, 'guard producer and recomputed raw verdicts must agree');
  assert.deepEqual(checked.missing, [], 'guard rows must not be missing');
  assert.deepEqual(checked.forged, [], 'guard rows must not be forged');
  assert.equal(checked.pass, true, 'all raw guard verdicts must pass');
  assert.deepEqual(
    verdictProblems(report.results),
    [],
    'strict mandatory and applicability verdicts'
  );
}

const extensionPath = fileURLToPath(
  new URL('./harness/extension', import.meta.resolve('@ai-ecoverse/slicc-shared-web/package.json'))
);

async function guardBrowser(
  startup,
  { privileged, extension, target, slot, siteIsolationOff, signal, deadlineAt }
) {
  const controller = new AbortController();
  const started = await deadline(
    () =>
      startup.start(
        [
          ...browserArgs({ privileged: privileged || extension, siteIsolationOff }),
          ...(extension ? ['--enable-unsafe-extension-debugging'] : []),
        ],
        { target, slot, signal: AbortSignal.any([signal, controller.signal]), deadlineAt }
      ),
    'browser-startup',
    Math.max(1, deadlineAt - Date.now()),
    (error) => controller.abort(error)
  );
  const browser = {
    ...started,
    close: () =>
      deadline(
        () => started.close(),
        'browser-close',
        cleanupGrace(deadlineAt + 30000),
        (error) => controller.abort(error)
      ),
  };
  if (!extension) return browser;
  let raw;
  try {
    raw = await deadline(
      () => rawSocket(browser.wsUrl),
      'extension-socket-startup',
      Math.max(1, deadlineAt - Date.now()),
      (error) => controller.abort(error)
    );
    const { id } = await deadline(
      () =>
        raw.send('Extensions.loadUnpacked', {
          path: extensionPath,
          enableInIncognito: true,
        }),
      'extension-load',
      Math.max(1, deadlineAt - Date.now()),
      (error) => controller.abort(error)
    );
    assert.equal(id, 'ankmbodmgimjemnojmeddfcdlbocolob');
    return browser;
  } catch (error) {
    await cleanup([() => browser.close()], error);
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
  requireChrome();
  const selectedTargets = partitions[id];
  assert.ok(selectedTargets, 'guard partition must be declared');
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
  const setupDeadline = started + Number(process.env.SLICC_GUARD_TIMEOUT ?? 900000) - 35000;
  const remaining = () => Math.max(1, setupDeadline - Date.now());
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
  const parallel = Number(process.env.SLICC_GUARD_PARALLEL ?? 4);
  const diagnostics = [];
  const tracedClients = new WeakMap();
  const activeTraceEvents = new Set();
  const recordedRows = [];
  const restoreClients = new Map();
  const restoreLedgers = new Map();
  const exitClients = new Map();
  const startup = browserStartup(launchBrowser, (retry) => {
    process.stdout.write(`chrome-startup-retry ${JSON.stringify(retry)}\n`);
  });
  const executeTarget = async (target) => {
    let fixture;
    const browsers = new Map();
    const selected = {
      ...set,
      vectors: set.vectors
        .filter((vector) => vector.targets.includes(target))
        .map((vector) => ({ ...vector, targets: [target] })),
    };
    const failures = [];
    try {
      fixture = await acquire(
        (signal) => fixtureServer({ signal }),
        'fixture-startup',
        (resource) => resource.close(),
        remaining()
      );
      requireRedirectRegistration(fixture);
      const targets = origins(fixture);
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
          discovery: true,
          clientExit: true,
          siteIsolationOff: true,
          inboundDelay: true,
          forwardAudit: true,
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
          const siteIsolationOff = vector.requires?.includes('siteIsolationOff') === true;
          const key = `${slot}/${siteIsolationOff ? 'shared' : 'isolated'}`;
          if (!browsers.has(key))
            browsers.set(
              key,
              guardBrowser(startup, {
                privileged: vector.id.startsWith('C8.'),
                extension: selected.vectors.some((row) => row.id.startsWith('C9.')),
                target,
                slot,
                siteIsolationOff,
                signal: t.signal,
                deadlineAt: setupDeadline,
              })
            );
          const browser = await browsers.get(key);
          if (vector.requires?.includes('inboundDelay')) {
            const made = await acquire(
              (signal) =>
                inboundClients(browser.wsUrl, targets.client, {
                  variant,
                  detachDelay,
                  signal,
                  deadlineAt: setupDeadline,
                  protectedOrigin: targets[target],
                  ordinaryOrigin: targets.ordinary,
                }),
              'inbound-clients-startup',
              (resource) => resource.close(),
              remaining()
            );
            restoreClients.set(`${vector.id}/${target}/${variant}`, made.client.messages);
            restoreLedgers.set(`${vector.id}/${target}/${variant}`, made.forwardLedger);
            startup.markRowStarted();
            return made;
          }
          const trace = ['C5.document', 'C5.continue-request'].includes(vector.id)
            ? await guardDiagnostics(browser.wsUrl, { id: vector.id, target, variant, slot })
            : null;
          let client;
          try {
            client = await acquire(
              (signal) =>
                recordedClient((onMessage) =>
                  guarded(trace?.url ?? browser.wsUrl, targets.client, {
                    variant,
                    detachDelay,
                    onMessage: (message) => {
                      onMessage(message);
                      trace?.onMessage(message);
                    },
                    signal,
                    deadlineAt: setupDeadline,
                  })
                ),
              'guarded-client-startup',
              (resource) => resource.close(),
              remaining()
            );
          } catch (error) {
            await cleanup([() => trace?.close()], error);
          }
          if (trace) {
            trace.wrap(client);
            tracedClients.set(client.events, trace);
            activeTraceEvents.add(client.events);
            diagnostics.push(trace.record);
          }
          if (vector.id === 'X1.after-exit') {
            const key = `${vector.id}/${target}/${variant}`;
            if (!exitClients.has(key))
              exitClients.set(key, {
                actors: [],
                ordinary: targets.ordinary,
                protectedOrigin: targets[target],
              });
            exitClients.get(key).actors.push(recordX1Client(client));
          }
          let unguarded;
          try {
            unguarded = await acquire(
              (signal) =>
                recordedClient((onMessage) =>
                  observer(browser.wsUrl, onMessage, { signal, deadlineAt: setupDeadline })
                ),
              'observer-startup',
              (resource) => resource.close(),
              remaining()
            );
          } catch (error) {
            try {
              await cleanup(
                [() => client.close(), () => trace?.close(), () => browser.close()],
                error
              );
            } finally {
              browsers.delete(key);
            }
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
                await cleanup([() => client.close(), () => unguarded.close()]);
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
          if (row.id === 'X1.after-exit') {
            const exit = exitClients.get(`${row.id}/${row.target}/${row.variant}`);
            row.x1Privacy = x1Privacy(exit?.actors, exit ?? {});
          }
          if (protectedBfcacheRows.has(row.id))
            row.guardedPrivacy = bfcachePrivacy(
              row,
              restoreClients.get(`${row.id}/${row.target}/${row.variant}`)
            );
          if (row.id === 'C8.protected-bfcache-restore.history')
            row.guardedForwardingAudit = restoreLedgers
              .get(`${row.id}/${row.target}/${row.variant}`)
              ?.historyAudit();
          recordedRows.push(structuredClone(row));
          if (process.env.SLICC_GUARD_PROGRESS)
            process.stdout.write(`row ${row.id} ${row.target} ${row.variant}\n`);
          if ([row.control, row.guarded, row.live].includes('fail'))
            process.stdout.write(`${JSON.stringify(row)}\n`);
        },
      });
      return execution.results;
    } catch (error) {
      failures.push(error);
    } finally {
      await cleanup(
        [
          ...[...browsers.values()].map((browser) => async () => (await browser).close()),
          () => fixture?.close(),
        ],
        ...failures
      );
    }
  };
  const targetNames = [...new Set(set.vectors.flatMap((vector) => vector.targets))];
  const targetParallel = Number(process.env.SLICC_GUARD_TARGET_PARALLEL ?? 2);
  const settled = [];
  for (let index = 0; index < targetNames.length; index += targetParallel) {
    settled.push(
      ...(await Promise.allSettled(
        targetNames.slice(index, index + targetParallel).map(executeTarget)
      ))
    );
  }
  const completed = settled
    .filter((value) => value.status === 'fulfilled')
    .flatMap((value) => value.value);
  const completedTargets = new Set(completed.map((row) => row.target));
  const results = [
    ...completed,
    ...recordedRows.filter((row) => !completedTargets.has(row.target)),
  ];
  const report = summary({
    set,
    results,
    guard: 'slicc-cdp',
    sha:
      process.env.SLICC_GUARD_SHA ??
      execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(),
  });
  const recorded = new Set(report.results.map(rowKey));
  const missing = new Set(report.missing.map(rowKey));
  report.missing.push(
    ...rowsFor(selectedTargets).filter(
      (row) => !recorded.has(rowKey(row)) && !missing.has(rowKey(row))
    )
  );
  report.partition = id;
  report.startupRetries = startup.retries;
  report.selection = { targets: selectedTargets };
  report.transport =
    process.env.SLICC_GUARD_TRANSPORT === 'wasi' ? 'wasi-kernel-cdp' : 'native-websocket';
  const problems = verdictProblems(report.results);
  const ms = Date.now() - started;
  const output = `${report.line}\nPartition ${id}: ${selectedTargets.join(', ')}\nRuntime: ${(ms / 1000).toFixed(1)}s\nStrict verdicts: ${problems.length ? `${problems.length} problems` : 'every mandatory row PASS or its declared applicability skip'}\n\n${table(report.results)}\n`;
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
  const availability = settled.flatMap((result, index) =>
    result.status === 'rejected'
      ? [
          {
            target: targetNames[index],
            phase: 'target-execution-or-cleanup',
            error: normalizeError(result.reason),
          },
        ]
      : []
  );
  await writeFile(
    `${file}.availability.json`,
    JSON.stringify({ type: 'HARNESS_AVAILABILITY', failures: availability }, null, 2)
  );
  for (const error of availability)
    process.stderr.write(`HARNESS-AVAILABILITY ${JSON.stringify(error)}\n`);
  assert.deepEqual(availability, [], 'all target invocations and cleanup must succeed');
  assert.deepEqual(
    report.results.map(rowKey).sort(),
    rowsFor(selectedTargets).map(rowKey).sort(),
    `partition ${id} must record every published row of its targets, once`
  );
  assertGuardReport(report, id, report.sha);
  assert.deepEqual(report.missing, [], 'all published rows must be present');
  assert.equal(report.observerSkipped, 0);
  assert.deepEqual(problems, [], 'every row is PASS or its declared not-applicable skip');
  assert.equal(report.pass, true, report.line);
}
