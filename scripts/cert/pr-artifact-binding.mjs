import { prIdentity } from './pr-common.mjs';

export function artifactNames(plan) {
  const run = `${plan.run}-${plan.attempt}`;
  const pack = `cert-pr-pack-${plan.pr}-${plan.head}-${plan.attempt}`;
  const names = {
    self: `cert-pr-self-${run}`,
    plan: `cert-pr-plan-${run}`,
    'pack-base': `${pack}-base`,
    'pack-candidate': `${pack}-candidate`,
    signed: `cert-pr-signed-${run}`,
    assets: `cert-pr-assets-${run}`,
    record: `cert-pr-produced-record-${run}`,
    report: `cert-pr-record-${plan.pr}-${plan.head}-${plan.attempt}`,
  };
  for (const label of [
    'linux-amd64',
    'linux-arm64',
    'windows-amd64',
    'windows-arm64',
    'darwin-amd64',
    'darwin-arm64',
  ])
    names[`native-${label}`] = `cert-pr-native-${run}-${label}`;
  for (const which of ['rebuild', 'guard', 'integration', 'bios-inputs', 'bios', 'review'])
    names[which] = `cert-pr-evidence-${run}-${which}`;
  for (const [role, platform] of [
    ['assets-linux', 'Linux'],
    ['assets-macos', 'macOS'],
  ])
    names[role] = `cert-pr-evidence-${run}-assets-${platform}`;
  for (const side of ['base', 'candidate']) {
    names[`bios-${side}-report`] = `${side}-canary`;
    for (const shard of ['packages-update-cdp', 'guard'])
      for (const kind of ['shard', 'files'])
        names[`bios-${side}-${kind}-${shard}`] = `${side}-canary-${kind}-canary-${shard}`;
  }
  return names;
}

export function artifactDigest(value) {
  if (typeof value !== 'string') return null;
  return /^(?:sha256:)?([a-f0-9]{64})$/.exec(value)?.[1] ?? null;
}

export function artifactId(value) {
  const text = String(value ?? '');
  return /^[1-9]\d*$/.test(text) && Number.isSafeInteger(Number(text)) ? text : null;
}

function consumerRoles(names, roles, phase, emit) {
  const required =
    phase === 'record' || phase === 'report'
      ? Object.keys(names).filter(
          (role) => role !== 'report' && (phase !== 'record' || role !== 'record')
        )
      : roles;
  if (
    !['consumer', 'record', 'report'].includes(phase) ||
    !Array.isArray(required) ||
    required.some((role) => !Object.hasOwn(names, role)) ||
    new Set(required).size !== required.length
  ) {
    emit('FAIL', 'invalid artifact consumer scope');
    return [];
  }
  if (!required.length) emit('INCOMPLETE', 'explicit artifact consumer roles required');
  return required;
}

function bindingIdentity(plan, bindings, emit) {
  if (bindings?.schema !== 1 || !Array.isArray(bindings.artifacts))
    emit('INCOMPLETE', 'trusted producer job-output bindings missing or malformed');
  else if (!bindings.head || !bindings.repository || !bindings.run || !bindings.attempt)
    emit('INCOMPLETE', 'producer binding plan identity unavailable');
  else if (JSON.stringify(prIdentity(bindings)) !== JSON.stringify(prIdentity(plan)))
    emit('FAIL', 'producer bindings belong to another plan');
}

function producerRow(row, ids, emit) {
  const id = artifactId(row.id);
  const digest = artifactDigest(row.digest);
  const present = (value) => value !== undefined && value !== null && value !== '';
  if (!id || !digest) emit('INCOMPLETE', `producer upload id/digest missing: ${row.role}`);
  if ((present(row.id) && !id) || (present(row.digest) && !digest))
    emit('FAIL', `malformed producer upload id/digest: ${row.role}`);
  if (id && ids.has(id)) emit('FAIL', `duplicate producer artifact id: ${id}`);
  if (id) ids.add(id);
  return { ...row, id, digest };
}

function producerRows(plan, bindings, names, emit) {
  bindingIdentity(plan, bindings, emit);
  const byRole = new Map();
  const ids = new Set();
  for (const row of Array.isArray(bindings?.artifacts) ? bindings.artifacts : []) {
    if (
      !row ||
      !Object.hasOwn(names, row.role) ||
      row.name !== names[row.role] ||
      byRole.has(row.role)
    ) {
      emit('FAIL', 'unexpected, duplicate or renamed producer artifact binding');
      continue;
    }
    byRole.set(row.role, producerRow(row, ids, emit));
  }
  return byRole;
}

function inventoryRows(actual, names, emit) {
  if (!Array.isArray(actual)) emit('INCOMPLETE', 'complete run artifact inventory unavailable');
  const inventory = Array.isArray(actual) ? actual : [];
  const allowed = new Set(Object.values(names));
  const seenNames = new Set();
  const seenIds = new Set();
  for (const item of inventory) {
    if (!item || !allowed.has(item.name)) {
      emit('FAIL', `unexpected run artifact: ${item?.name ?? 'malformed'}`);
      continue;
    }
    const id = artifactId(item.id);
    if (!id || seenNames.has(item.name) || seenIds.has(id))
      emit('FAIL', `duplicate or malformed run artifact: ${item.name}`);
    seenNames.add(item.name);
    seenIds.add(id);
  }
  return inventory;
}

function apiBinding(plan, role, bound, item, emit) {
  const digest = artifactDigest(item.digest);
  if (!digest) emit('INCOMPLETE', `API artifact digest unavailable: ${role}`);
  if (item.digest && !digest) emit('FAIL', `malformed API artifact digest: ${role}`);
  const source = item.workflow_run;
  if (source?.id === undefined || source?.head_sha === undefined)
    emit('INCOMPLETE', `API artifact run/head unavailable: ${role}`);
  const mismatch =
    artifactId(item.id) !== bound.id ||
    (digest && digest !== bound.digest) ||
    (source?.id !== undefined && String(source.id) !== String(plan.run)) ||
    (source?.head_sha !== undefined && source.head_sha !== plan.head);
  if (mismatch) emit('FAIL', `producer/API artifact identity or digest mismatch: ${role}`);
  if (item.expired !== false) emit('INCOMPLETE', `artifact expired or expiry unavailable: ${role}`);
  return (
    !mismatch &&
    digest === bound.digest &&
    String(source?.id) === String(plan.run) &&
    source?.head_sha === plan.head &&
    item.expired === false
  );
}

function matchArtifact(plan, role, name, bound, inventory, emit) {
  if (!bound?.id || !bound.digest) {
    emit('INCOMPLETE', `producer binding required: ${role}`);
    return null;
  }
  const items = inventory.filter((item) => item?.name === name);
  if (items.length !== 1) {
    emit('INCOMPLETE', `exact artifact required: ${role}`);
    return null;
  }
  return apiBinding(plan, role, bound, items[0], emit)
    ? { role, name, id: bound.id, digest: bound.digest }
    : null;
}

export function artifactFindings(plan, bindings, actual, { roles = [], phase = 'consumer' } = {}) {
  const failures = [];
  const incomplete = [];
  const emit = (status, reason) => (status === 'FAIL' ? failures : incomplete).push(reason);
  const names = artifactNames(plan);
  const required = consumerRoles(names, roles, phase, emit);
  if (
    plan.mode !== 'pr' ||
    !artifactId(plan.run) ||
    !artifactId(plan.attempt) ||
    !Number.isSafeInteger(plan.pr) ||
    plan.pr < 1 ||
    !/^[a-f0-9]{40}$/.test(plan.head ?? '')
  )
    emit('FAIL', 'invalid artifact plan identity');
  const byRole = producerRows(plan, bindings, names, emit);
  const inventory = inventoryRows(actual, names, emit);
  const matched = required
    .map((role) => matchArtifact(plan, role, names[role], byRole.get(role), inventory, emit))
    .filter(Boolean);
  return {
    schema: 1,
    ...prIdentity(plan),
    which: 'artifacts',
    status: failures.length ? 'FAIL' : incomplete.length ? 'INCOMPLETE' : 'PASS',
    phase,
    roles: required,
    expectedNames: Object.values(names),
    matched,
    failures: [...new Set(failures)],
    incomplete: [...new Set(incomplete)],
  };
}

export async function artifactInventory(api, run) {
  if (!artifactId(run)) throw new Error('invalid artifact run');
  const rows = [];
  let total;
  for (let page = 1; page <= 10; page++) {
    const result = await api(`actions/runs/${run}/artifacts?per_page=100&page=${page}`);
    if (
      !Array.isArray(result.artifacts) ||
      !Number.isSafeInteger(result.total_count) ||
      result.total_count < 0 ||
      (total !== undefined && result.total_count !== total)
    )
      throw new Error('incomplete or changing run artifact inventory');
    total = result.total_count;
    rows.push(...result.artifacts);
    if (rows.length === total) return rows;
    if (rows.length > total || result.artifacts.length !== 100) break;
  }
  throw new Error('run artifact inventory truncated');
}
