import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { skipReason } from '@ai-ecoverse/slicc-browsers/guard-driver.mjs';
import { summary } from '@ai-ecoverse/slicc-browsers/guard-runner';
import { guardRowKey, guardShardPlan } from './cert/guard-shards.mjs';
import { suitePolicy } from './cert/suite-policy.mjs';
import { integrationArguments } from './integration-selection.mjs';

export function assertPartitionVerdicts(vectors, report, head) {
  const checked = summary({ set: vectors, results: report.results, guard: 'slicc-cdp', sha: head });
  assert.equal(report.guard, 'slicc-cdp', 'guard scope binding');
  assert.equal(report.set, checked.set, 'guard corpus binding');
  assert.equal(report.pass, true, 'guard producer verdict must pass');
  assert.equal(checked.pass, true, 'recomputed guard verdict must pass');
  for (const row of report.results) {
    const vector = vectors.vectors.find((vector) => vector.id === row.id);
    const legs = [row.control, row.guarded, row.live];
    const mandatory =
      row.id.startsWith('C2.redirect-') || /^C4\.iframe-preattach\..*\.shared$/.test(row.id);
    if (legs.every((leg) => leg === 'pass')) continue;
    assert.equal(mandatory, false, `${row.id} must pass without skips`);
    if (vector?.kind === 'text' && legs.every((leg) => leg === 'not-run')) continue;
    const canonical = skipReason(vector, row.target, 'slicc-cdp');
    const originUnavailable =
      row.reason === 'not-applicable:no-ordinary-same-site-origin' &&
      JSON.stringify(vector).includes('{ordinary-same-site}');
    assert.ok(
      legs.every((leg) => leg === 'skip') &&
        typeof row.reason === 'string' &&
        row.reason.startsWith('not-applicable') &&
        (row.reason === canonical || originUnavailable),
      `${row.id} has no allowed applicability skip`
    );
  }
}

export function planPartitions(vectors) {
  const plan = guardShardPlan(vectors);
  const union = plan.shards.flatMap((shard) => shard.keys).sort();
  assert.equal(new Set(union).size, union.length, 'guard partitions overlap');
  assert.deepEqual(union, plan.keys, 'guard partitions must cover the entire published universe');
  return plan;
}

export function assertPartitionUnion(vectors, documents, head, run) {
  const plan = planPartitions(vectors);
  assert.equal(documents.length, plan.shards.length, 'every guard partition report is required');
  const seen = new Set();
  const union = [];
  for (const document of documents) {
    const shard = plan.shards.find((shard) => shard.id === document.id);
    assert.ok(shard && !seen.has(document.id), 'unknown or duplicate guard partition');
    seen.add(document.id);
    assert.equal(document.head, head, 'guard partition head binding');
    assert.equal(document.run, run, 'guard partition run binding');
    assert.equal(document.report.sha, head, 'guard report subject binding');
    assert.deepEqual([...document.report.selection.targets].sort(), [...shard.targets].sort());
    const keys = document.report.results.map(guardRowKey).sort();
    assert.equal(new Set(keys).size, keys.length, 'guard partition repeats rows');
    assert.deepEqual(keys, shard.keys, 'guard partition must record every assigned row');
    assertPartitionVerdicts(
      {
        ...vectors,
        vectors: vectors.vectors
          .map((vector) => ({
            ...vector,
            targets: vector.targets.filter((target) => shard.targets.includes(target)),
          }))
          .filter((vector) => vector.targets.length),
      },
      document.report,
      head
    );
    union.push(...keys);
  }
  assert.equal(new Set(union).size, union.length, 'guard executed partitions overlap');
  assert.deepEqual(
    union.sort(),
    plan.keys,
    'guard executed row union must equal the full universe'
  );
  return union.length;
}

async function main() {
  const vectors = JSON.parse(
    await readFile(
      new URL(import.meta.resolve('@ai-ecoverse/slicc-browsers/guard-vectors.json')),
      'utf8'
    )
  );
  const [mode, argument, directory] = process.argv.slice(2);
  const head = process.env.GITHUB_SHA;
  const run = process.env.GITHUB_RUN_ID;
  if (mode === 'run') {
    const shard = planPartitions(vectors).shards.find((shard) => shard.id === argument);
    assert.ok(
      shard && directory && head && run,
      'guard CI requires a declared partition and subject'
    );
    await mkdir(directory, { recursive: true });
    const report = path.join(directory, 'guard.md');
    const files = await integrationArguments('guard');
    const outcome = spawnSync(
      process.execPath,
      ['scripts/test-integration.mjs', '--test-concurrency=1', ...files],
      {
        stdio: 'inherit',
        env: {
          ...process.env,
          SLICC_GUARD_TARGETS: JSON.stringify(shard.targets),
          SLICC_GUARD_TARGET_PARALLEL: String(
            Math.floor(suitePolicy.guardTargetParallel / suitePolicy.fileWorkers)
          ),
          SLICC_GUARD_PARALLEL: String(suitePolicy.guardSlotParallel),
          SLICC_GUARD_TIMEOUT: String(suitePolicy.testTimeoutMs),
          SLICC_GUARD_SHA: head,
          SLICC_GUARD_REPORT: report,
        },
      }
    );
    await writeFile(
      path.join(directory, 'subject.json'),
      JSON.stringify({ id: shard.id, head, run, targets: shard.targets })
    );
    if (outcome.error) throw outcome.error;
    process.exitCode = outcome.status === 0 ? 0 : 1;
    console.log(`guard-ci: ${outcome.status === 0 ? 'PASS' : 'FAIL'} ${argument}`);
  } else if (mode === 'union') {
    assert.ok(argument && head && run, 'guard union requires directory and subject');
    const documents = await Promise.all(
      planPartitions(vectors).shards.map(async ({ id }) => ({
        ...JSON.parse(await readFile(path.join(argument, id, 'subject.json'), 'utf8')),
        report: JSON.parse(await readFile(path.join(argument, id, 'guard.md.json'), 'utf8')),
      }))
    );
    console.log(`guard-ci: PASS union ${assertPartitionUnion(vectors, documents, head, run)} rows`);
  } else throw new Error('unknown guard CI mode');
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  await main().catch((error) => {
    console.error(`guard-ci: FAIL ${error.message}`);
    process.exitCode = 1;
  });
