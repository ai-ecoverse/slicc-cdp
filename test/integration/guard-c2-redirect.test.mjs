import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test, { before } from 'node:test';
import { fixtureServer } from '@ai-ecoverse/slicc-browsers/fixture';
import { run, summary } from '@ai-ecoverse/slicc-browsers/guard-runner';
import { launchBrowser } from './browser.mjs';
import {
  recordedClient,
  redirectIds,
  redirectSelection,
  requireRedirectRegistration,
} from './guard-c2-support.mjs';
import { guarded, observer } from './guard-client.mjs';
import { browserArgs, origins } from './guard-fixture.mjs';

const published = JSON.parse(
  await readFile(
    new URL(import.meta.resolve('@ai-ecoverse/slicc-browsers/guard-vectors.json')),
    'utf8'
  )
);
const selected = redirectSelection(published);

before(() => {
  for (const target of [[], ['--target', 'wasm32-wasip1']])
    execFileSync(
      'cargo',
      ['build', '-p', 'slicc-cdp-client', '--example', 'guard_bridge', ...target],
      { stdio: 'inherit' }
    );
});

for (const transport of ['native', 'wasi']) {
  for (const id of redirectIds) {
    const rowSet = { ...selected, vectors: selected.vectors.filter((vector) => vector.id === id) };
    test(`published ${id} redirects and fragments through ${transport}`, {
      timeout: 300000,
    }, async (t) => {
      const fixture = requireRedirectRegistration(await fixtureServer());
      const targets = origins(fixture);
      const previous = process.env.SLICC_GUARD_TRANSPORT;
      let browser;
      try {
        process.env.SLICC_GUARD_TRANSPORT = transport;
        browser = await launchBrowser(browserArgs());
        const execution = await run(rowSet, {
          fixture,
          targets,
          capabilities: {
            scope: 'slicc-cdp',
            autoAttach: true,
            browserSession: true,
            browserContexts: true,
            discovery: true,
          },
          variant: Object.keys(selected.variants),
          parallel: 1,
          clientFactory: async ({ variant, detachDelay }) => {
            t.signal.throwIfAborted();
            const client = await recordedClient((onMessage) =>
              guarded(browser.wsUrl, targets.client, { variant, detachDelay, onMessage })
            );
            let unguarded;
            try {
              unguarded = await recordedClient((onMessage) => observer(browser.wsUrl, onMessage));
            } catch (error) {
              await client.close();
              throw error;
            }
            return {
              client,
              unguarded,
              close: async () => {
                await client.close();
                await unguarded.close();
              },
            };
          },
          log: (row) => console.log(JSON.stringify({ ...row, transport })),
        });
        const report = summary({
          set: rowSet,
          results: execution.results,
          guard: 'slicc-cdp',
          sha: execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(),
        });
        report.transport = transport;
        const file = `${process.env.SLICC_GUARD_REPORT ?? path.join(tmpdir(), 'slicc-cdp-guard-runner-report.md')}.c2-${transport}-${id}.json`;
        await writeFile(file, JSON.stringify(report, null, 2));
        console.log(`C2-FOCUS ${report.line}`);
        {
          const rows = report.results.filter((row) => row.id === id);
          const expected = rowSet.vectors.flatMap((vector) =>
            vector.targets.flatMap((target) =>
              Object.keys(rowSet.variants).map((variant) => `${target}/${variant}`)
            )
          );
          assert.deepEqual(
            rows.map((row) => `${row.target}/${row.variant}`).sort(),
            expected.sort(),
            `${id}: target or variant missing or duplicated`
          );
          for (const row of rows)
            assert.deepEqual(
              [row.control, row.guarded, row.live],
              ['pass', 'pass', 'pass'],
              `${transport} ${id} ${row.variant}: ${JSON.stringify(row)}`
            );
        }
        assert.deepEqual(report.missing, []);
        assert.equal(report.observerSkipped, 0);
        assert.equal(report.pass, true, report.line);
      } finally {
        await browser?.close();
        await fixture.close();
        if (previous === undefined) delete process.env.SLICC_GUARD_TRANSPORT;
        else process.env.SLICC_GUARD_TRANSPORT = previous;
      }
    });
  }
}
