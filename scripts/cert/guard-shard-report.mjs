import { summary } from '@ai-ecoverse/slicc-browsers/guard-runner';
import { recordGuardShards } from './guard-shards.mjs';

export function aggregateGuardReports(vectors, plan, documents) {
  const coverage = recordGuardShards(plan, documents);
  const reports = documents.flatMap((document) => {
    const report = document?.suite?.guardReport?.report;
    return report ? [report] : [];
  });
  for (const report of reports)
    if (!/^[a-f0-9]{40}$/.test(report.sha ?? '') || typeof report.set !== 'string')
      coverage.incomplete.push('guard shard source or corpus absent');
    else if (
      report.sha !== plan.head ||
      report.set !== `@ai-ecoverse/slicc-browsers@${vectors.setVersion}`
    )
      coverage.failures.push('guard shard executed another source or corpus');
  if (
    reports.some(
      (report) =>
        Array.isArray(report.results) &&
        report.results.some((row) => row && [row.control, row.guarded, row.live].includes('fail'))
    )
  )
    coverage.failures.push('guard shard rows failed');
  coverage.status = coverage.failures.length
    ? 'FAIL'
    : coverage.incomplete.length
      ? 'INCOMPLETE'
      : 'PASS';
  const report = reports.length
    ? {
        ...summary({
          set: vectors,
          results: reports.flatMap((report) =>
            Array.isArray(report.results)
              ? report.results.filter((row) => row && typeof row === 'object')
              : []
          ),
          guard: 'slicc-cdp',
          sha: plan.head,
        }),
        selection: { targets: plan.targets, allTargets: plan.targets },
        transport: 'native-websocket',
        startupRetries: reports.flatMap((report) => report.startupRetries ?? []),
      }
    : null;
  return { coverage, report };
}
