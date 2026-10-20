const partitions = [
  { id: 'a', targets: ['seven', 'ordinary', 'chrome-newtab', 'chrome-passwords'] },
  { id: 'b', targets: ['vault', 'chrome-new-tab-page', 'chrome-settings'] },
  { id: 'c', targets: ['vault-x', 'own', 'chrome-search'] },
  { id: 'd', targets: ['apex', 'chrome-extension-page', 'file-root', 'network-error'] },
];

export const guardRowKey = (row) => JSON.stringify([row.id, row.target, row.variant]);

export function guardShardPlan(set) {
  const targets = [...new Set(set.vectors.flatMap((vector) => vector.targets))].sort();
  const assigned = partitions.flatMap((shard) => shard.targets);
  if (
    new Set(assigned).size !== assigned.length ||
    targets.some((target) => !assigned.includes(target))
  )
    throw new Error('published guard targets must have exactly one fixed shard assignment');
  const rows = set.vectors.flatMap((vector) =>
    vector.targets.flatMap((target) =>
      Object.keys(set.variants).map((variant) => ({ id: vector.id, target, variant }))
    )
  );
  const keys = rows.map(guardRowKey).sort();
  if (new Set(keys).size !== keys.length) throw new Error('published guard row keys are ambiguous');
  const shards = partitions.map((shard) => ({
    id: shard.id,
    targets: shard.targets.filter((target) => targets.includes(target)),
    keys: rows
      .filter((row) => shard.targets.includes(row.target))
      .map(guardRowKey)
      .sort(),
  }));
  for (const shard of shards) shard.expectedRows = shard.keys.length;
  if (shards.some((shard) => !shard.targets.length || !shard.keys.length))
    throw new Error('each fixed guard shard must contain published targets and rows');
  return { schema: 1, targets, keys, shards };
}

function checkRows(report, shard, issues) {
  const actual = Array.isArray(report?.results) ? report.results.map(guardRowKey).sort() : null;
  if (
    !actual ||
    new Set(actual).size !== actual.length ||
    JSON.stringify(actual) !== JSON.stringify(shard.keys)
  )
    issues.incomplete.push('guard shard ' + shard.id + ' lacks its exact complete row set');
  if (!Array.isArray(report?.selection?.targets))
    issues.incomplete.push('guard shard ' + shard.id + ' executed target selection absent');
  else if (
    JSON.stringify([...report.selection.targets].sort()) !==
    JSON.stringify([...shard.targets].sort())
  )
    issues.failures.push('guard shard ' + shard.id + ' executed target selection mismatch');
  if (Array.isArray(report?.missing) && report.missing.length)
    issues.incomplete.push('guard shard ' + shard.id + ' reports missing rows');
  return actual?.length ?? 0;
}

function checkBinding(plan, shard, document, issues) {
  if (!Array.isArray(document.targets))
    issues.incomplete.push('guard shard ' + shard.id + ' target assignment absent');
  else if (
    JSON.stringify([...document.targets].sort()) !== JSON.stringify([...shard.targets].sort())
  )
    issues.failures.push('guard shard ' + shard.id + ' target assignment mismatch');
  if (document.head !== plan.head || document.run !== plan.run)
    issues.failures.push('guard shard ' + shard.id + ' measurement subject mismatch');
}

export function guardSetupTimeoutCount(report) {
  if (!Array.isArray(report?.results)) return null;
  const setupError =
    /CDP timeout Target\.(createTarget|attachToTarget)|debugging port.*did not answer|browser websocket failed/i;
  return report.results.filter((row) =>
    (row.errors ?? []).some((error) => setupError.test(String(error)))
  ).length;
}

function checkExecution(shard, document, report, issues) {
  const status = document.suite?.status;
  if (status === 'INCOMPLETE' || !document.suite)
    issues.incomplete.push('guard shard ' + shard.id + ' execution incomplete');
  else if (status === 'FAIL' || (report && report.pass === false))
    issues.failures.push('guard shard ' + shard.id + ' failed');
  else if (status !== 'PASS')
    issues.incomplete.push('guard shard ' + shard.id + ' execution status unavailable');
  if (report && document.setupTimeoutCount !== guardSetupTimeoutCount(report))
    issues.failures.push('guard shard ' + shard.id + ' setup timeout count mismatch');
  if (!Number.isFinite(document.wallTimeMs) || document.wallTimeMs < 0)
    issues.incomplete.push('guard shard ' + shard.id + ' wall time unavailable');
  if (
    report &&
    (!Number.isSafeInteger(document.setupTimeoutCount) || document.setupTimeoutCount < 0)
  )
    issues.incomplete.push('guard shard ' + shard.id + ' setup timeout count unavailable');
}

export function recordGuardShards(plan, documents) {
  const issues = { failures: [], incomplete: [] };
  const seen = new Set();
  const metrics = [];
  if (
    JSON.stringify(plan.shards.map((shard) => shard.id).sort()) !==
    JSON.stringify(partitions.map((shard) => shard.id))
  )
    issues.failures.push('fixed guard shard plan is incomplete or ambiguous');
  if (plan.shards.some((shard) => shard.expectedRows !== shard.keys.length))
    issues.failures.push('guard shard expected row count differs from declared keys');
  const planned = plan.shards.flatMap((shard) => shard.keys).sort();
  if (
    new Set(planned).size !== planned.length ||
    JSON.stringify(planned) !== JSON.stringify(plan.keys)
  )
    issues.failures.push('guard shard plan does not partition the full row universe');
  for (const document of documents) {
    const shard = plan.shards.find((shard) => shard.id === document.id);
    if (!shard || seen.has(document.id)) {
      issues.failures.push('unknown or duplicate guard shard ' + document.id);
      continue;
    }
    seen.add(document.id);
    const report = document.suite?.guardReport?.report;
    const rows = checkRows(report, shard, issues);
    checkBinding(plan, shard, document, issues);
    checkExecution(shard, document, report, issues);
    metrics.push({
      id: shard.id,
      targets: shard.targets,
      wallTimeMs: document.wallTimeMs,
      suiteTimeMs: document.suite?.elapsedMs,
      setupTimeoutCount: document.setupTimeoutCount,
      status: document.suite?.status ?? 'INCOMPLETE',
      rows,
      expectedRows: shard.expectedRows,
    });
  }
  for (const shard of plan.shards)
    if (!seen.has(shard.id)) issues.incomplete.push('guard shard ' + shard.id + ' missing');
  return {
    schema: 1,
    status: issues.failures.length ? 'FAIL' : issues.incomplete.length ? 'INCOMPLETE' : 'PASS',
    ...issues,
    targets: plan.targets,
    rows: plan.keys.length,
    metrics,
    measurementOnly: true,
  };
}
