import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import {
  guardFiles,
  integrationArguments,
  integrationFiles,
  redirectFiles,
} from '../../scripts/integration-selection.mjs';

const root = path.resolve(import.meta.dirname, '../..');

test('every matrix and focused job consumes the shared selector rather than a shadow file list', async () => {
  const directory = await mkdtemp(path.join(tmpdir(), 'ci-manifest-regression-'));
  const preloader = path.join(directory, 'preload.mjs');
  const capture = path.join(directory, 'captured.json');
  await writeFile(
    preloader,
    `import {registerHooks} from 'node:module';globalThis.manifestGroups=[];registerHooks({resolve(s,c,n){if((c.parentURL?.endsWith('/scripts/guard-ci.mjs')||c.parentURL?.endsWith('/scripts/redirect-ci.mjs'))&&s==='./integration-selection.mjs')return {url:'manifest-fixture:selection',shortCircuit:true};if((c.parentURL?.endsWith('/scripts/guard-ci.mjs')||c.parentURL?.endsWith('/scripts/redirect-ci.mjs'))&&s==='node:child_process')return {url:'manifest-fixture:process',shortCircuit:true};return n(s,c);},load(u,c,n){if(u==='manifest-fixture:selection')return {format:'module',source:'export async function integrationArguments(group){globalThis.manifestGroups.push(group);return ["test/integration/guard-runner.test.mjs","test/integration/new-owned-guard.test.mjs"]}',shortCircuit:true};if(u==='manifest-fixture:process')return {format:'module',source:'import {writeFileSync} from "node:fs";export function spawnSync(exe,args){writeFileSync(process.env.MANIFEST_CAPTURE,JSON.stringify({groups:globalThis.manifestGroups,args}));return {status:0}}',shortCircuit:true};return n(u,c);}});`
  );
  try {
    const result = spawnSync(
      process.execPath,
      ['--import', preloader, 'scripts/guard-ci.mjs', 'run', 'a', directory],
      {
        cwd: root,
        encoding: 'utf8',
        env: {
          ...process.env,
          GITHUB_SHA: 'manifest-subject',
          GITHUB_RUN_ID: 'manifest-run',
          MANIFEST_CAPTURE: capture,
        },
      }
    );
    assert.equal(result.status, 0, result.stderr);
    const saved = JSON.parse(await readFile(capture, 'utf8'));
    assert.deepEqual(saved.groups, ['guard']);
    assert.deepEqual(saved.args.slice(2), [
      'test/integration/guard-runner.test.mjs',
      'test/integration/new-owned-guard.test.mjs',
    ]);
    const redirected = spawnSync(
      process.execPath,
      ['--import', preloader, 'scripts/redirect-ci.mjs'],
      { cwd: root, encoding: 'utf8', env: { ...process.env, MANIFEST_CAPTURE: capture } }
    );
    assert.equal(redirected.status, 0, redirected.stderr);
    const redirectArgs = JSON.parse(await readFile(capture, 'utf8'));
    assert.deepEqual(redirectArgs.groups, ['redirects']);
    assert.deepEqual(redirectArgs.args, saved.args);
    const workflow = await readFile(path.join(root, '.github/workflows/ci.yml'), 'utf8');
    const focused = workflow.slice(
      workflow.indexOf('  redirect-regressions:\n'),
      workflow.indexOf('  integration-chrome-latest:\n')
    );
    assert.ok(
      focused.includes('node scripts/gate.mjs redirect-regressions -- node scripts/redirect-ci.mjs')
    );
    for (const file of redirectFiles)
      assert.equal(
        focused.includes(file),
        false,
        'hard-coded focused file list shadows the selector'
      );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test('a new file assigned only in the shared manifest remains in the exact union of job files', async () => {
  const original = await integrationArguments('all');
  for (const manifest of [guardFiles, redirectFiles]) {
    const added = 'test/integration/new-manifest-only.test.mjs';
    manifest.push(added);
    try {
      const inventory = [...original, added];
      const jobs = ['integration', 'guard', 'redirects'].map((group) =>
        integrationFiles(inventory, group)
      );
      assert.deepEqual(jobs.flat().sort(), inventory.sort());
      assert.equal(new Set(jobs.flat()).size, inventory.length);
    } finally {
      manifest.pop();
    }
  }
});
