import { skipReason } from '@ai-ecoverse/slicc-browsers/guard-driver.mjs';
import { summary } from '@ai-ecoverse/slicc-browsers/guard-runner';

export function syntheticReport(set, sha = 'synthetic-report') {
  const results = set.vectors.flatMap((vector) =>
    vector.targets.flatMap((target) =>
      Object.keys(set.variants).map((variant) => {
        const reason = skipReason(vector, target, 'slicc-cdp');
        const state = vector.kind === 'text' ? 'not-run' : reason ? 'skip' : 'pass';
        return {
          id: vector.id,
          target,
          variant,
          transport: 'cdp',
          control: state,
          guarded: state,
          live: state,
          ...(state === 'skip' ? { reason } : {}),
          ...(vector.id.startsWith('C8.protected-bfcache-restore')
            ? {
                guardedPrivacy: {
                  measured: true,
                  markerExposed: false,
                  protectedPathExposed: false,
                },
              }
            : {}),
          ...(vector.id === 'C8.protected-bfcache-restore.history'
            ? {
                guardedForwardingAudit: { id: 'history-trigger', complete: true, forwarded: false },
              }
            : {}),
          ...(vector.id === 'X1.after-exit'
            ? {
                x1Privacy: {
                  measured: true,
                  markerExposed: false,
                  protectedExposed: false,
                  screenshotExposed: false,
                },
              }
            : {}),
        };
      })
    )
  );
  return summary({ set, results, guard: 'slicc-cdp', sha });
}
