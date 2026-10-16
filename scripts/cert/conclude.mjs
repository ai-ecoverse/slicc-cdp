import { readFileSync } from 'node:fs';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { summary } from '@ai-ecoverse/slicc-browsers/guard-runner';
import { identity, json, name, save, semver } from './common.mjs';

const { values } = parseArgs({
  options: { evidence: { type: 'string' }, out: { type: 'string' } },
});
const plan = json(path.join(values.evidence, 'plan.json'));
if (!semver.test(plan.version) || !/^[a-f0-9]{40}$/.test(plan.commit))
  throw new Error('invalid plan identity');
const vectorSet = JSON.parse(
  readFileSync(
    new URL(import.meta.resolve('@ai-ecoverse/slicc-browsers/guard-vectors.json')),
    'utf8'
  )
);
if (vectorSet.setVersion !== plan.vectors) throw new Error('record runner pin differs from plan');
const incomplete = [...(plan.issues ?? [])];
const failures = [];
const read = (which) => {
  try {
    const item = json(path.join(values.evidence, `${which}.json`));
    if (JSON.stringify(identity(item)) !== JSON.stringify(identity(plan))) {
      incomplete.push(`${which}: identity/pins/run attempt mismatch`);
      return null;
    }
    if (item.which !== which || !['PASS', 'FAIL', 'INCOMPLETE'].includes(item.status)) {
      incomplete.push(`${which}: no valid result`);
      return null;
    }
    if (item.status === 'INCOMPLETE')
      incomplete.push(`${which}: ${item.reason ?? 'required evidence incomplete'}`);
    if (item.status === 'FAIL')
      failures.push(`${which}: ${item.failures?.join('; ') || item.reason || 'check failed'}`);
    return item;
  } catch {
    incomplete.push(`${which}: missing results`);
    return null;
  }
};
const registry = read('registry');
const rebuild = read('rebuild');
const guard = read('guard');
const integration = read('integration');
const linux = read('assets-linux');
const macos = read('assets-macos');
if (
  !registry?.provenance?.workflow ||
  !registry?.provenance?.subject ||
  !registry?.signatures ||
  !registry?.ancestry ||
  registry?.crates?.length !== 4
)
  incomplete.push('registry: missing provenance, signature or crate results');
if (
  !/^[a-f0-9]{128}$/.test(registry?.sha512 ?? '') ||
  !registry?.tagCommit ||
  !registry?.provenance?.commit ||
  !registry?.provenance?.builder ||
  !registry?.provenance?.subject?.digest?.sha512
)
  incomplete.push('registry: missing fresh tag or signed subject results');
if (registry?.tagCommit && registry.tagCommit !== plan.commit)
  failures.push('registry: fresh tag mismatch');
if (
  registry?.signatures &&
  (registry.signatures.exit === null || registry.signatures.signal || registry.signatures.error)
)
  incomplete.push('registry: signature command did not complete');
if (!/^[a-f0-9]{64}$/.test(registry?.sha256 ?? '') || registry?.gitHead !== plan.commit)
  incomplete.push('registry: missing authoritative hash/gitHead');
for (const [which, item] of [
  ['rebuild', rebuild],
  ['guard', guard],
  ['integration', integration],
])
  if (item?.sha256 && registry?.sha256 && item.sha256 !== registry.sha256)
    failures.push(`${which}: checked another tarball`);
if (
  rebuild &&
  (!Array.isArray(rebuild.files) ||
    rebuild.files.length !== 2 ||
    !['bin/playwright-cli.wasm', 'bin/curlwright.wasm'].every((file) =>
      rebuild.files.some(
        (row) =>
          row.file === file &&
          /^[a-f0-9]{64}$/.test(row.built) &&
          /^[a-f0-9]{64}$/.test(row.published)
      )
    ))
)
  incomplete.push('rebuild: missing wasm comparison results');
else if (rebuild?.files.some((row) => row.built !== row.published))
  failures.push('rebuild: wasm comparison did not pass');
for (const leg of ['positive', 'negative']) {
  const execution = guard?.executions?.[leg];
  const report = execution?.report;
  if (
    !execution?.attempts?.length ||
    !report?.results?.length ||
    report.missing?.length !== 0 ||
    report.observerSkipped !== 0 ||
    execution.status === 'INCOMPLETE'
  )
    incomplete.push(`guard: ${leg} missing required row execution`);
  const version = leg === 'positive' ? plan.version : plan.baseline.version;
  const commit = leg === 'positive' ? plan.commit : plan.baseline.commit;
  if (execution && (execution.version !== version || execution.commit !== commit))
    incomplete.push(`guard: ${leg} ran another version`);
  if (report?.results?.length) {
    const checked = summary({
      set: vectorSet,
      results: report.results,
      guard: 'slicc-cdp',
      sha: commit,
    });
    if (checked.missing.length || report.set !== checked.set || report.sha !== commit)
      incomplete.push(`guard: ${leg} rows/set/commit mismatch`);
    for (const row of report.results) {
      const vector = vectorSet.vectors.find((item) => item.id === row.id);
      if (
        !vector ||
        row.transport !== 'cdp' ||
        !Object.keys(vectorSet.variants).includes(row.variant) ||
        !vector.targets.includes(row.target)
      )
        incomplete.push(`guard: ${leg} unexpected row ${row.id}`);
      if (vector?.kind !== 'text' && [row.control, row.guarded, row.live].includes('not-run'))
        incomplete.push(`guard: ${leg} unrun executable row ${row.id}`);
      if (
        row.control === 'skip' &&
        !(vector?.skip?.target === row.target || vector?.observable?.[row.target] === false)
      )
        incomplete.push(`guard: ${leg} skipped required row ${row.id}`);
      if (
        leg === 'negative' &&
        row.control === 'fail' &&
        !guard.expected.includes(`${row.id}/${row.target}/${row.variant}`)
      )
        incomplete.push(`guard: undeclared negative control failure ${row.id}`);
    }
    if (leg === 'negative') {
      const observed = report.results
        .filter((row) => [row.control, row.guarded, row.live].includes('fail'))
        .map((row) => `${row.id}/${row.target}/${row.variant}`)
        .sort();
      if (JSON.stringify(observed) !== JSON.stringify(guard.expected))
        incomplete.push('guard: raw NEGATIVE rows do not match declared fixes');
    }
    const keys = report.results.map(
      (row) => `${row.id}/${row.target}/${row.variant}/${row.transport}`
    );
    if (new Set(keys).size !== keys.length) incomplete.push(`guard: ${leg} duplicate rows`);
    if (leg === 'positive' && checked.missing.length === 0 && !checked.pass)
      failures.push('guard: recomputed positive summary failed');
  }
  if (leg === 'positive' && report && report.missing?.length === 0 && report.pass !== true)
    failures.push('guard: positive did not pass');
}
if (guard && JSON.stringify(guard.expected) !== JSON.stringify(guard.failures))
  incomplete.push('guard: NEGATIVE failures lack matching declared fixes');
if (
  !integration?.executions?.positive?.files?.length ||
  !integration?.executions?.positive?.rows?.length ||
  !integration?.executions?.positive?.attempts?.length
)
  incomplete.push('integration: suite without results');
for (const leg of ['positive', 'negative']) {
  const execution = integration?.executions?.[leg];
  if (!execution) continue;
  if (
    execution.status === 'INCOMPLETE' ||
    execution.missing?.length ||
    execution.rows?.some((row) => row.skip || row.todo)
  )
    incomplete.push(`integration: ${leg} incomplete or skipped results`);
  for (const file of execution.files ?? [])
    if (
      !execution.rows?.some(
        (row) =>
          row.file?.endsWith(file) &&
          row.name &&
          row.name !== row.file &&
          ['test:pass', 'test:fail'].includes(row.type)
      )
    )
      incomplete.push(`integration: ${leg} file without results ${file}`);
  if (leg === 'positive' && execution.status === 'FAIL')
    failures.push('integration: positive suite failed');
}
if (plan.fixes.length && !integration?.executions?.negative?.attempts?.length)
  incomplete.push('integration: NEGATIVE did not run');
for (const fix of plan.fixes) {
  const result = integration?.fixes?.find((item) => item.commit === fix.commit);
  if (
    result?.status !== 'PASS' ||
    !result.results?.some(
      (test) =>
        test.current?.some((row) => row.type === 'test:pass' && !row.skip && !row.todo) &&
        test.previous?.some(
          (row) =>
            row.type === 'test:fail' &&
            row.failureType === 'testCodeFailure' &&
            !/running 0 tests|could not compile|ERR_MODULE_NOT_FOUND|Cannot find module/.test(
              row.error ?? ''
            )
        )
    )
  )
    incomplete.push(`integration: ${fix.commit} lacks positive and meaningful NEGATIVE evidence`);
}
const inventory = json(path.join(import.meta.dirname, 'inventory.json'));
if (
  !inventory.every((file) =>
    linux?.checksums?.files?.some((row) => row.file === file && row.status === 'PASS')
  )
)
  incomplete.push('assets-linux: missing complete release checksum evidence');
if (
  !inventory
    .filter((file) => file.includes('-darwin-'))
    .every((file) =>
      macos?.files?.some(
        (row) =>
          row.file === file &&
          row.signature?.display &&
          row.signature?.strict &&
          row.signature?.notarisation
      )
    )
)
  incomplete.push('assets-macos: missing all four signature/notarisation results');
for (const file of macos?.files ?? []) {
  const signature = file.signature;
  const steps = [signature?.display, signature?.strict, signature?.notarisation];
  if (
    typeof signature?.valid !== 'boolean' ||
    !signature?.identifier ||
    !signature?.team ||
    typeof signature?.display?.stderr !== 'string'
  )
    incomplete.push(`assets-macos: missing signature identity result for ${file.file}`);
  if (steps.some((step) => !step || !Number.isInteger(step.exit) || step.signal || step.error))
    incomplete.push(`assets-macos: unfinished signature step for ${file.file}`);
  else if (steps.some((step) => step.exit !== 0) || signature?.valid === false)
    failures.push(`assets-macos: signature verification failed for ${file.file}`);
}
for (const [which, result] of Object.entries(plan.upstream ?? {}))
  if (result !== 'success') incomplete.push(`${which}: upstream ${result}`);
if (!plan.upstream || Object.keys(plan.upstream).length !== 4)
  incomplete.push('missing required upstream job outcomes');
const status = failures.length ? 'FAIL' : incomplete.length ? 'INCOMPLETE' : 'PASS';
const certified = status === 'PASS';
const sha256 = registry?.sha256 ?? null;
const line = certified
  ? `certified-candidate ${name}@${plan.version} sha256=${sha256} gitHead=${plan.commit} guard=GUARD-PASS(slicc-cdp@${plan.commit}; vectors @ai-ecoverse/slicc-browsers@${plan.vectors})`
  : `cert-ci: ${status} ${name}@${plan.version} (${[...failures, ...incomplete].join('; ')})`;
const record = {
  ...identity(plan),
  status,
  certified,
  sha256,
  gitHead: registry?.gitHead,
  line,
  failures,
  incomplete,
  evidence: values.evidence,
  runUrl: `https://github.com/ai-ecoverse/slicc-cdp/actions/runs/${plan.run}`,
};
save(values.out, record);
console.log(line);
process.exitCode = certified ? 0 : 1;
