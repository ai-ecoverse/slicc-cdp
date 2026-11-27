import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';
import { pathToFileURL } from 'node:url';
import { exercise, fixture, inputs, refused, request } from './signed-lease-fixture.mjs';

const pkg = '/node_modules/@ai-ecoverse/slicc-cdp';
const wasm = 'bin/playwright-cli.wasm';
const selector = 'ws://127.0.0.1:9222/slicc/lease/v1/018a7d5f-ab01-4abb-bacf-a57dd66c12e4';

async function install(kernel) {
  const data = await readFile(path.resolve('target/wasm32-wasip1/debug/playwright-cli.wasm'));
  const manifest = {
    name: '@ai-ecoverse/slicc-cdp',
    slicc: { abi: 'wasi', commands: { 'playwright-cli': { wasm } } },
  };
  await kernel.writeFile(`${pkg}/package.json`, JSON.stringify(manifest));
  await kernel.writeFile(`${pkg}/${wasm}`, data);
}

test('the WASI client never reaches an ordinary host, even when its lease selector is missing, malformed, or points at ordinary CDP', {
  timeout: 90000,
}, async () => {
  const { createNodeKernel } = await import('@ai-ecoverse/slicc-kernel/node');
  let calls = 0;
  const kernel = await createNodeKernel({
    cdp: async () => {
      calls++;
      throw new Error('unexpected raw host');
    },
  });
  try {
    await install(kernel);
    refused(
      await kernel.run(['playwright-cli', ...request], {
        cwd: '/',
        env: { SLICC_CDP_URL: selector },
      }),
      'E_UNAVAILABLE'
    );
    for (const fake of [
      '',
      'ws://localhost:9222/devtools/browser/1123',
      `${selector}?runtime=other`,
      selector.replace('127.0.0.1', 'external.example'),
      selector,
    ]) {
      const r = await kernel.run(['playwright-cli', ...request], {
        cwd: '/',
        env: { SLICC_CDP_LEASE_URL: fake },
      });
      refused(r, 'E_UNAVAILABLE');
    }
    assert.equal(calls, 0);
  } finally {
    kernel.terminate();
  }
});

async function leased(t, body) {
  const module = process.env.SLICC_TEST_LEASE_KERNEL_MODULE;
  const api = await import(
    module ? pathToFileURL(path.resolve(module)).href : '@ai-ecoverse/slicc-kernel/node'
  );
  if (!module && api.CDP_LEASE_VERSION !== 1) {
    t.skip(
      'pinned kernel has no lease facade; set SLICC_TEST_LEASE_KERNEL_MODULE to test a lease-enabled build'
    );
    return;
  }
  assert.equal(api.CDP_LEASE_VERSION, 1);
  const job = fixture();
  const tag = 'fixture-job';
  const kernel = await api.createNodeKernel({ cdpLease: { tag, adapter: job.adapter } });
  let client;
  try {
    await install(kernel);
    client = await api.attachKernel(await kernel.connect({ tag }));
    await body((argv) => client.run(['playwright-cli', ...argv], { cwd: '/' }), job);
  } finally {
    await client?.close();
    kernel.terminate();
  }
}

test('the WASI command path uses a dedicated tagged lease kernel when one is available', {
  timeout: 120000,
}, async (t) => {
  await leased(t, (run, job) => exercise(run, job, { leaseKernel: true }));
});

test('the lease kernel refuses click and fill before the adapter is called', {
  timeout: 120000,
}, async (t) => {
  await leased(t, async (run, job) => {
    for (const [op, ...args] of inputs) {
      refused(await run(['--lease', 'L1', op, ...args]), 'E_UNSUPPORTED', 'L1');
    }
    assert.deepEqual(job.calls, []);
  });
});
