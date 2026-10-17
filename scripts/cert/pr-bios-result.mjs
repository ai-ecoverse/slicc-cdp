const packageName = '@ai-ecoverse/slicc-cdp';
const kinds = ['pass', 'skip', 'fail', 'regression', 'both', 'flaky'];
const lists = { fail: 'failures', regression: 'regressions', both: 'both', flaky: 'flaky' };

export function biosCalls(plan, pin, subjects) {
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

function completion(side, result, pin, emit, flags) {
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
  if (!counts.pass || counts.skip) emit('INCOMPLETE', `${side}: unexecuted or skipped smoke`);
  if (counts.flaky) emit('INCOMPLETE', `${side}: flaky smoke`);
  if (side === 'candidate' && (counts.regression || counts.fail))
    emit('FAIL', 'candidate: regressions or failing smoke');
  if (!result.ok) emit('INCOMPLETE', `${side}: producer did not accept the smoke`);
  if (counts.both) flags.push(`bios ${side}: ${counts.both} failures also present on bios pins`);
  if (side === 'base' && counts.regression)
    flags.push(`bios base: ${counts.regression} regressions against bios pins`);
}

export function biosFindings(bios, pin, plan, subjects) {
  const failures = [];
  const incomplete = [];
  const flags = [];
  const emit = (status, message) =>
    (status === 'FAIL' ? failures : incomplete).push(`bios: ${message}`);
  if (!/^[a-f0-9]{40}$/.test(pin.commit ?? '') || bios?.pin?.commit !== pin.commit) {
    emit('INCOMPLETE', 'pinned canary workflow_call evidence pending or another producer');
    return { failures, incomplete, flags };
  }
  const calls = biosCalls(plan, pin, subjects);
  const artifacts = [];
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
    completion(side, result, pin, emit, flags);
  }
  if (artifacts.length === 2 && artifacts[0] === artifacts[1])
    emit('INCOMPLETE', 'base and candidate result artifacts are not distinct');
  return { failures, incomplete, flags };
}
