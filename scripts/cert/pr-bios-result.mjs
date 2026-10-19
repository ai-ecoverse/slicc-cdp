import { isDeepStrictEqual } from 'node:util';
import {
  artifactDigest,
  artifactFindings,
  artifactId,
  artifactNames,
} from './pr-artifact-binding.mjs';
import { biosAdvisoryLedger } from './pr-bios-ledger.mjs';
import { pairedBiosFindings } from './pr-bios-pair.mjs';
import { biosExecutionRowFindings } from './pr-bios-rows.mjs';

const packageName = '@ai-ecoverse/slicc-cdp';
const kinds = ['pass', 'skip', 'fail', 'regression', 'both', 'flaky'];
const lists = { fail: 'failures', regression: 'regressions', both: 'both', flaky: 'flaky' };
const producerArtifact = (bindings, role) =>
  (Array.isArray(bindings?.artifacts) ? bindings.artifacts : []).find((row) => row?.role === role);

export function selectBiosPin(pin, bound = false) {
  if (!bound) return pin;
  if (!/^[a-f0-9]{40}$/.test(pin.bindingCommit ?? ''))
    throw new Error('reviewed binding producer pin required');
  return { ...pin, commit: pin.bindingCommit, bindingSchema: 1 };
}

export function biosCalls(plan, pin, subjects, producerBindings) {
  return Object.fromEntries(
    ['base', 'candidate'].map((side) => [
      side,
      {
        label: side,
        overrides: '[]',
        runner: 'ubuntu-latest',
        shards: JSON.stringify(pin.shards),
        'bios-ref': pin.commit,
        'candidate-package': packageName,
        'candidate-artifact': `cert-pr-pack-${plan.pr}-${plan.head}-${plan.attempt}-${side}`,
        'candidate-run': plan.run,
        'candidate-sha256': subjects[side]?.sha256,
        'candidate-repo': plan.repository,
        ...(pin.bindingSchema === 1
          ? {
              'candidate-binding': 'strict',
              'candidate-artifact-id':
                artifactId(producerArtifact(producerBindings, `pack-${side}`)?.id) ?? '',
              'candidate-artifact-digest':
                artifactDigest(producerArtifact(producerBindings, `pack-${side}`)?.digest) ?? '',
            }
          : {}),
        repeat: 3,
      },
    ])
  );
}

function artifactUrl(value, prefix) {
  return (
    typeof value === 'string' &&
    value.startsWith(`${prefix}/artifacts/`) &&
    /^\d+$/.test(value.slice(`${prefix}/artifacts/`.length))
  );
}

function bindings(side, bios, pin, plan, subject, call, emit, artifacts) {
  const result = bios.results[side];
  const prefix = `https://github.com/${plan.repository}/actions/runs/${plan.run}`;
  if (
    !bios.calls?.[side] ||
    Object.entries(call).some(([key, value]) => bios.calls[side][key] !== value)
  )
    emit('INCOMPLETE', `${side}: bound workflow inputs missing or different`);
  if (
    result.biosRef !== pin.commit ||
    result.run !== 'canary' ||
    result.repeat !== 3 ||
    result.url !== prefix
  )
    emit('INCOMPLETE', `${side}: producer/ref/run/repeat identity mismatch`);
  if (result.artifact?.name !== `${side}-canary` || !artifactUrl(result.artifact?.url, prefix))
    emit('INCOMPLETE', `${side}: labelled result artifact missing or bound to another run`);
  else artifacts.push(result.artifact.url);
  const overrides = Array.isArray(result.overrides) ? result.overrides : [];
  const override = overrides.find((row) => row?.name === packageName);
  if (
    overrides.length !== 1 ||
    override?.version !== subject.manifest.version ||
    !/^[a-f0-9]{64}$/.test(override?.sha256 ?? '')
  )
    emit('INCOMPLETE', `${side}: exact package override missing`);
  else if (override.sha256 !== subject.sha256)
    emit('FAIL', `${side}: canary override pack digest mismatch`);
  const candidates = Array.isArray(result.candidates) ? result.candidates : [];
  const selected = candidates.filter(
    (row) => row?.package === `${packageName}@${subject.manifest.version}`
  );
  const binding = `candidate (artifact ${plan.run}/${call['candidate-artifact']}, sha256 ${subject.sha256})`;
  if (selected.length !== 1 || selected[0].via !== 'artifact' || selected[0].status !== binding)
    emit('INCOMPLETE', `${side}: candidate artifact/run/digest binding missing or different`);
}

function rowLists(side, result, pin, emit) {
  for (const [kind, list] of Object.entries(lists)) {
    const rows = result[list];
    if (
      !Array.isArray(rows) ||
      rows.length !== Math.min(result.counts[kind], 100) ||
      rows.some(
        (row) =>
          !row ||
          !pin.shards.includes(row.shard) ||
          typeof row.test !== 'string' ||
          !row.test ||
          typeof row.log !== 'string'
      )
    )
      emit('INCOMPLETE', `${side}: ${list} rows/counts incomplete`);
  }
}

function completion(side, result, pin, emit, flags, paired) {
  if (
    typeof result.ok !== 'boolean' ||
    !Array.isArray(result.missingShards) ||
    result.missingShards.length ||
    !Array.isArray(result.unexplainedExits) ||
    result.unexplainedExits.length
  )
    emit('INCOMPLETE', `${side}: shard completion or exit evidence missing`);
  const counts = result.counts;
  if (!counts || kinds.some((kind) => !Number.isSafeInteger(counts[kind]) || counts[kind] < 0)) {
    emit('INCOMPLETE', `${side}: result counts malformed`);
    return;
  }
  rowLists(side, result, pin, emit);
  if (!kinds.some((kind) => counts[kind]) || counts.skip)
    emit('INCOMPLETE', `${side}: unexecuted or skipped smoke`);
  if (
    counts.flaky &&
    !(
      side === 'base' &&
      paired.advisories.length === counts.flaky &&
      !paired.failures.length &&
      !paired.incomplete.length
    )
  )
    emit('INCOMPLETE', `${side}: flaky smoke`);

  if (counts.both) flags.push(`bios ${side}: ${counts.both} failures also present on bios pins`);
  if (side === 'base' && counts.regression)
    flags.push(`bios base: ${counts.regression} regressions against bios pins`);
}

const supplied = (value) => value !== undefined && value !== null && value !== '';

function manifestRows(output, side, emit) {
  let rows = output?.artifacts;
  if (typeof rows === 'string' && rows) {
    try {
      rows = JSON.parse(rows);
    } catch {
      emit('FAIL', `${side}: malformed producer manifest JSON`);
    }
  }
  if (Array.isArray(rows)) return rows;
  emit(supplied(rows) ? 'FAIL' : 'INCOMPLETE', `${side}: producer manifest unavailable`);
  return [];
}

function uploadBinding(row, role, plan, producerBindings, ids, emit) {
  const id = artifactId(row.id);
  const digest = artifactDigest(row.digest);
  for (const [field, normalize] of [
    ['id', artifactId],
    ['digest', artifactDigest],
  ])
    if (!normalize(row[field]))
      emit(supplied(row[field]) ? 'FAIL' : 'INCOMPLETE', `${role}: upload ${field} unavailable`);
  if (id && ids.has(id)) emit('FAIL', `${role}: duplicate upload id`);
  if (id) ids.add(id);
  if (String(row.attempt ?? '') !== String(plan.attempt))
    emit(supplied(row.attempt) ? 'FAIL' : 'INCOMPLETE', `${role}: wrong or absent upload attempt`);
  const bound = producerArtifact(producerBindings, role);
  if (
    bound &&
    ((id && id !== artifactId(bound.id)) || (digest && digest !== artifactDigest(bound.digest)))
  )
    emit('FAIL', `${role}: callee output differs from trusted producer binding`);
}

function reportBinding(output, side, producerBindings, emit) {
  const report = producerArtifact(producerBindings, `bios-${side}-report`);
  for (const [field, normalize, expected] of [
    ['artifact-id', artifactId, artifactId(report?.id)],
    ['artifact-digest', artifactDigest, artifactDigest(report?.digest)],
  ]) {
    const value = output?.[field];
    if (!supplied(value)) emit('INCOMPLETE', `${side}: direct report ${field} missing`);
    else if (!normalize(value) || (expected && normalize(value) !== expected))
      emit('FAIL', `${side}: direct report ${field} mismatch`);
  }
}

function labelledUploads(plan, side, output, producerBindings, ids, emit) {
  const wanted = Object.entries(artifactNames(plan)).filter(([role]) =>
    role.startsWith(`bios-${side}-`)
  );
  const byName = new Map(wanted.map(([role, name]) => [name, role]));
  const seen = new Set();
  for (const row of manifestRows(output, side, emit)) {
    const role = byName.get(row?.name);
    const kind = role?.slice(`bios-${side}-`.length).split('-')[0];
    if (!role || row.role !== kind || seen.has(role)) {
      emit('FAIL', `${side}: unexpected, duplicate or renamed upload`);
      continue;
    }
    seen.add(role);
    uploadBinding(row, role, plan, producerBindings, ids, emit);
  }
  for (const [role] of wanted)
    if (!seen.has(role)) emit('INCOMPLETE', `${role}: producer upload missing`);
  reportBinding(output, side, producerBindings, emit);
}

export function biosBindingFindings(plan, outputs, producerBindings, inventory) {
  const roles = Object.keys(artifactNames(plan)).filter((role) => role.startsWith('bios-'));
  const checked = artifactFindings(plan, producerBindings, inventory, {
    roles: ['pack-base', 'pack-candidate', ...roles],
  });
  const emit = (status, reason) =>
    (status === 'FAIL' ? checked.failures : checked.incomplete).push(`bios uploads: ${reason}`);
  const ids = new Set();
  for (const side of ['base', 'candidate'])
    labelledUploads(plan, side, outputs?.[side], producerBindings, ids, emit);
  checked.failures = [...new Set(checked.failures)];
  checked.incomplete = [...new Set(checked.incomplete)];
  checked.status = checked.failures.length
    ? 'FAIL'
    : checked.incomplete.length
      ? 'INCOMPLETE'
      : 'PASS';
  return checked;
}

function resultBinding(bios, side, emit) {
  const value = bios?.outputs?.[side]?.result;
  if (value === undefined || value === null || value === '') {
    emit('INCOMPLETE', `${side}: callee result output absent`);
    return;
  }
  try {
    const result = typeof value === 'string' ? JSON.parse(value) : value;
    if (!isDeepStrictEqual(result, bios?.results?.[side]))
      emit('FAIL', `${side}: parsed result differs from callee result output`);
  } catch {
    emit('FAIL', `${side}: malformed callee result output`);
  }
}

export function biosFindings(bios, pin, plan, subjects, context = {}) {
  const failures = [];
  const incomplete = [];
  const flags = [];
  const emit = (status, message) =>
    (status === 'FAIL' ? failures : incomplete).push(`bios: ${message}`);
  const calls = biosCalls(plan, pin, subjects, context.bindings);
  if (pin.bindingSchema === 1) {
    const checked = biosBindingFindings(plan, bios?.outputs, context.bindings, context.inventory);
    for (const reason of checked.failures) emit('FAIL', reason);
    for (const reason of checked.incomplete) emit('INCOMPLETE', reason);
    for (const side of ['base', 'candidate']) {
      resultBinding(bios, side, emit);
      const reportId = artifactId(bios?.outputs?.[side]?.['artifact-id']);
      const url = bios?.results?.[side]?.artifact?.url;
      if (reportId && typeof url === 'string' && url.slice(url.lastIndexOf('/') + 1) !== reportId)
        emit('FAIL', `${side}: result URL differs from report upload output`);
    }
  }
  if (!/^[a-f0-9]{40}$/.test(pin.commit ?? '') || bios?.pin?.commit !== pin.commit) {
    emit('INCOMPLETE', 'pinned canary workflow_call evidence pending or another producer');
    return { failures, incomplete, flags };
  }
  const artifacts = [];
  const paired = pairedBiosFindings(bios?.results, pin, bios?.reports, bios?.rows);
  const actual = biosExecutionRowFindings(
    plan,
    pin,
    bios?.rows,
    subjects,
    context.inventory,
    context.rows
  );
  failures.push(...actual.failures);
  incomplete.push(...actual.incomplete);
  for (const side of ['base', 'candidate']) {
    const subject = subjects[side];
    const result = bios?.results?.[side];
    if (
      !subject ||
      !/^[a-f0-9]{64}$/.test(subject.sha256 ?? '') ||
      !result ||
      typeof result !== 'object' ||
      Array.isArray(result)
    ) {
      emit('INCOMPLETE', `${side}: exact pack or canary result absent`);
      continue;
    }
    bindings(side, bios, pin, plan, subject, calls[side], emit, artifacts);
    completion(side, result, pin, emit, flags, paired);
  }
  if (artifacts.length === 2 && artifacts[0] === artifacts[1])
    emit('INCOMPLETE', 'base and candidate result artifacts are not distinct');
  failures.push(...paired.failures);
  incomplete.push(...paired.incomplete);
  flags.push(...paired.flags);
  let biosLedger = [];
  const advisories = failures.length || incomplete.length ? [] : paired.advisories;
  try {
    biosLedger = biosAdvisoryLedger(plan, pin, advisories, bios?.rows);
  } catch (error) {
    emit('INCOMPLETE', error.message);
  }
  return { failures, incomplete, flags, advisories, biosLedger };
}
