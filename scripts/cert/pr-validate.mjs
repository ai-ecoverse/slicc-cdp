import { isDeepStrictEqual } from 'node:util';
import { summary } from '@ai-ecoverse/slicc-browsers/guard-runner';
import { guardEvidenceProtocol, guardToolsProtocol } from './guard-protocol.mjs';
import { aggregateGuardReports } from './guard-shard-report.mjs';
import { guardShardPlan } from './guard-shards.mjs';
import { biosFindings } from './pr-bios-result.mjs';

export function validateSuite(execution, label, emit, candidate = true) {
  if (
    !Array.isArray(execution?.files) ||
    !Array.isArray(execution?.rows) ||
    !Array.isArray(execution?.attempts) ||
    !execution.files.length ||
    !execution.rows.length ||
    !execution.attempts.length ||
    execution.files.some((file) => typeof file !== 'string') ||
    execution.rows.some((row) => !row || typeof row !== 'object')
  ) {
    emit('INCOMPLETE', `${label}: execution absent`);
    return;
  }
  if (
    execution.status === 'INCOMPLETE' ||
    execution.missing?.length ||
    execution.rows.some(
      (row) =>
        row.skip ||
        row.todo ||
        ['cancelledByParent', 'testTimeoutFailure'].includes(row.failureType)
    )
  )
    emit('INCOMPLETE', `${label}: unfinished/skipped worker`);
  for (const file of execution.files) {
    if (
      !execution.rows.some(
        (row) =>
          row.worker === file &&
          ['test:pass', 'test:fail'].includes(row.type) &&
          row.name !== row.file
      )
    )
      emit('INCOMPLETE', `${label}: missing worker ${file}`);
  }
  if (
    candidate &&
    (execution.status === 'FAIL' || execution.rows.some((row) => row.type === 'test:fail'))
  )
    emit('FAIL', `${label}: candidate failed`);
}

function validateGuardRows(report, leg, vectors, emit) {
  const keys = report.results.map(
    (row) => `${row.id}/${row.target}/${row.variant}/${row.transport}`
  );
  if (new Set(keys).size !== keys.length) emit('INCOMPLETE', `guard ${leg}: duplicate rows`);
  for (const row of report.results) {
    const vector = vectors.vectors.find((item) => item.id === row.id);
    if (
      !vector ||
      row.transport !== 'cdp' ||
      !vector.targets.includes(row.target) ||
      !Object.hasOwn(vectors.variants, row.variant)
    )
      emit('INCOMPLETE', `guard ${leg}: unexpected row ${row.id}`);
    if (vector?.kind !== 'text' && [row.control, row.guarded, row.live].includes('not-run'))
      emit('INCOMPLETE', `guard ${leg}: unrun row ${row.id}`);
    if (
      row.control === 'skip' &&
      !(vector?.skip?.target === row.target || vector?.observable?.[row.target] === false)
    )
      emit('INCOMPLETE', `guard ${leg}: skipped row ${row.id}`);
  }
}

function validateGuardShards(execution, subject, plan, vectors, leg, emit, producingTools) {
  const protocol = guardEvidenceProtocol(execution, producingTools);
  if (protocol === 'LEGACY') return;
  if (protocol !== 'CURRENT') {
    emit('INCOMPLETE', `guard ${leg}: current shard protocol or verified legacy evidence absent`);
    return;
  }
  const expectedPlan = { ...guardShardPlan(vectors), head: subject.commit, run: plan.run };
  if (!isDeepStrictEqual(execution.guardShards.plan, expectedPlan))
    emit('INCOMPLETE', `guard ${leg}: shard plan contradicts independent corpus/source`);
  if (!Array.isArray(execution.shards)) emit('INCOMPLETE', `guard ${leg}: shard executions absent`);
  else {
    const aggregate = aggregateGuardReports(vectors, expectedPlan, execution.shards);
    for (const reason of aggregate.coverage.incomplete)
      emit('INCOMPLETE', `guard ${leg}: ${reason}`);
    for (const reason of aggregate.coverage.failures)
      if (
        leg === 'positive' ||
        !/^guard shard [a-d] failed$|^guard shard rows failed$/.test(reason)
      )
        emit('FAIL', `guard ${leg}: ${reason}`);
    if (
      !isDeepStrictEqual(execution.report, aggregate.report) ||
      !isDeepStrictEqual(execution.guardReport?.report, aggregate.report)
    )
      emit('FAIL', `guard ${leg}: aggregate differs from captured shard rows`);
  }
}

export function validateGuard(guard, plan, vectors, emit) {
  const producingTools = guardToolsProtocol(plan.tools);
  for (const leg of ['positive', 'negative']) {
    const execution = guard?.executions?.[leg];
    const subject =
      leg === 'positive' ? { version: plan.version, commit: plan.commit } : plan.baseline;
    validateSuite(execution, `guard ${leg}`, emit, leg === 'positive');
    validateGuardShards(execution, subject, plan, vectors, leg, emit, producingTools);
    const report = execution?.report;
    if (
      !Array.isArray(report?.results) ||
      !report.results.length ||
      report.results.some((row) => !row || typeof row !== 'object') ||
      report.observerSkipped !== 0 ||
      execution.commit !== subject.commit ||
      execution.version !== subject.version
    ) {
      emit('INCOMPLETE', `guard ${leg}: rows/source absent`);
      continue;
    }
    const recomputed = summary({
      set: vectors,
      results: report.results,
      guard: 'slicc-cdp',
      sha: subject.commit,
    });
    if (recomputed.missing.length || report.set !== recomputed.set || report.sha !== subject.commit)
      emit('INCOMPLETE', `guard ${leg}: row set/identity incomplete`);
    if (leg === 'positive' && !recomputed.pass) emit('FAIL', 'guard: recomputed candidate failed');
    validateGuardRows(report, leg, vectors, emit);
  }
}

export function validateBios(bios, pin, emit, plan, subjects, context, ledger) {
  const findings = biosFindings(bios, pin, plan, subjects, context);
  for (const reason of findings.failures) emit('FAIL', reason);
  for (const reason of findings.incomplete) emit('INCOMPLETE', reason);
  if (bios?.status === 'FAIL') emit('FAIL', 'bios: producer reported failure');
  if (
    bios?.rows !== undefined &&
    (!isDeepStrictEqual(bios.advisories, findings.advisories) ||
      !isDeepStrictEqual(bios.biosLedger, findings.biosLedger))
  )
    emit('FAIL', 'bios: producer advisory or ledger differs from independent classification');
  if (Array.isArray(ledger)) ledger.push(...(findings.biosLedger ?? []));
  return findings.flags;
}

export function validateMac(macos, inventory, emit) {
  const files = Array.isArray(macos?.files) ? macos.files : [];
  for (const file of inventory.filter((item) => item.includes('-darwin-'))) {
    const signature = files.find((item) => item?.file === file)?.signature;
    const steps = [signature?.display, signature?.strict, signature?.notarisation];
    if (
      steps.some(
        (step) =>
          !step ||
          !Number.isInteger(step.exit) ||
          step.signal ||
          step.error ||
          typeof step.stderr !== 'string'
      )
    ) {
      emit('INCOMPLETE', `macos: signature/notarisation unfinished ${file}`);
      continue;
    }
    const identifier = file.startsWith('playwright-cli')
      ? 'ai.sliccy.playwright-cli'
      : 'ai.sliccy.curlwright';
    if (
      steps.some((step) => step.exit !== 0) ||
      !signature.display.stderr.includes(`Identifier=${identifier}\n`) ||
      !signature.display.stderr.includes('Authority=Developer ID Application:') ||
      !signature.display.stderr.includes('TeamIdentifier=S8LB56P782\n') ||
      !signature.notarisation.stderr.includes('source=Notarized Developer ID') ||
      !/\baccepted\b/.test(signature.notarisation.stderr)
    )
      emit('FAIL', `macos: signature identity/strict/notarisation failed ${file}`);
  }
}
