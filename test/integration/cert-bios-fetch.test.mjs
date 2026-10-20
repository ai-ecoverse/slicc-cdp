import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readdirSync, readFileSync } from 'node:fs';
import test from 'node:test';

const fixture = new URL('./cert-bios-fetch.mjs', import.meta.url).href;
const snapshot = JSON.parse(
  readFileSync(new URL('./cert-bios-shard-catalog.json', import.meta.url))
);
const url = `https://raw.githubusercontent.com/${snapshot.repository}/${snapshot.commit}/scripts/canary/shards.json`;
const sha256 = '0876362e0660c59ec362b2e416c1bde0a46d252ebca7a4af569e1586a4994ae9';

function child(source) {
  return execFileSync(process.execPath, ['--input-type=module', '-e', source], {
    encoding: 'utf8',
  });
}

test('CLI catalog preload serves only the exact reviewed URL with independently asserted golden bytes', () => {
  assert.equal(snapshot.commit, '8d4e5d56bfdce208f853c8ea98628392787e31ea');
  assert.equal(snapshot.repository, 'ai-ecoverse/slicc-bios');
  assert.equal(snapshot.sha256, sha256);
  assert.equal(createHash('sha256').update(snapshot.content).digest('hex'), sha256);
  const observed = child(`
    globalThis.fetch = async () => { throw new Error('outbound network blocked'); };
    await import(${JSON.stringify(fixture)});
    const response = await fetch(${JSON.stringify(url)}, { signal: AbortSignal.timeout(1000) });
    process.stdout.write(JSON.stringify({ ok: response.ok, body: await response.text() }));
  `);
  assert.deepEqual(JSON.parse(observed), { ok: true, body: snapshot.content });
});

test('CLI catalog preload passes every other URL and its original options through unchanged', () => {
  const urls = [
    url.replace(snapshot.commit, 'main'),
    url.replace(snapshot.commit, 'f'.repeat(40)),
    url.replace(snapshot.repository, 'fixture/other'),
    `${url}?different=1`,
    url.replace('shards.json', 'other.json'),
    'http://127.0.0.1:9300/fixture-api',
  ];
  const observed = child(`
    import assert from 'node:assert/strict';
    const calls = [];
    const options = { headers: { 'fixture-header': 'kept' } };
    globalThis.fetch = async (input, actual) => {
      assert.equal(actual, options);
      calls.push(input);
      return new Response('passed through');
    };
    await import(${JSON.stringify(fixture)});
    for (const input of ${JSON.stringify(urls)})
      assert.equal(await (await fetch(input, options)).text(), 'passed through');
    process.stdout.write(JSON.stringify(calls));
  `);
  assert.deepEqual(JSON.parse(observed), urls);
});

test('the real pinned catalog reader succeeds with external fetching blocked under the test-only preload', () => {
  const reader = new URL('../../scripts/cert/pr-bios-shards.mjs', import.meta.url).href;
  const pin = JSON.parse(readFileSync(new URL('../../scripts/cert/pr-bios.json', import.meta.url)));
  const observed = child(`
    globalThis.fetch = async () => { throw new Error('outbound network blocked'); };
    await import(${JSON.stringify(fixture)});
    const { fetchBiosShardContract } = await import(${JSON.stringify(reader)});
    process.stdout.write(JSON.stringify(await fetchBiosShardContract(${JSON.stringify(pin)})));
  `);
  const contract = JSON.parse(observed);
  assert.equal(contract.status, 'PASS');
  assert.equal(contract.commit, snapshot.commit);
  assert.equal(contract.sha256, sha256);
  assert.deepEqual(contract.shards, pin.shards);
  assert.deepEqual(contract.incomplete, []);
});

test('the exact staged binding catalog stays unknown-shard INCOMPLETE with external fetching blocked', () => {
  const bound = JSON.parse(
    readFileSync(new URL('./cert-bios-bound-shard-catalog.json', import.meta.url))
  );
  const bytes = Buffer.from(bound.content);
  const boundHash = '04ddd94597619bcd1a66fdb745fcf699dc090d174c0fe7ea902d2cd8d0a420b4';
  assert.equal(bound.commit, 'c2cec127b935419049568f584093d7ef03ac0721');
  assert.equal(bound.repository, snapshot.repository);
  assert.equal(bound.sha256, boundHash);
  assert.equal(createHash('sha256').update(bytes).digest('hex'), boundHash);
  assert.equal(bound.blob, '9f46b195017a200e7348bf65dfd6686823ba441f');
  assert.equal(
    createHash('sha1').update(`blob ${bytes.length}\0`).update(bytes).digest('hex'),
    bound.blob
  );
  const reader = new URL('../../scripts/cert/pr-bios-shards.mjs', import.meta.url).href;
  const pin = JSON.parse(readFileSync(new URL('../../scripts/cert/pr-bios.json', import.meta.url)));
  assert.equal(pin.bindingCommit, bound.commit);
  const observed = child(`
    globalThis.fetch = async () => { throw new Error('outbound network blocked'); };
    await import(${JSON.stringify(fixture)});
    const { fetchBiosShardContract } = await import(${JSON.stringify(reader)});
    process.stdout.write(JSON.stringify(await fetchBiosShardContract(${JSON.stringify({ ...pin, commit: bound.commit })})));
  `);
  const contract = JSON.parse(observed);
  assert.equal(contract.status, 'INCOMPLETE');
  assert.equal(contract.commit, bound.commit);
  assert.equal(contract.sha256, boundHash);
  assert.deepEqual(contract.shards, pin.shards);
  assert.deepEqual(
    contract.incomplete,
    ['guard-prompt', 'guard-lagging'].map((name) => `unknown BIOS shard ${name} at ${bound.commit}`)
  );
});

test('the cert network sweep documents every matching source file', () => {
  const directory = new URL('./', import.meta.url);
  const audit = JSON.parse(readFileSync(new URL('./cert-network-audit.json', import.meta.url)));
  const requested =
    /raw\.githubusercontent|api\.github\.com|registry\.npmjs\.org|\bfetch\s*\(|https\.get/;
  const expanded =
    /https?:\/\/|\bfetch\b|\bhttps\b|npm|curl|wget|registry|GITHUB_API_URL|NPM_CONFIG/;
  const matched = readdirSync(directory)
    .filter((file) => /^cert-.*\.mjs$/.test(file))
    .filter((file) => {
      const source = readFileSync(new URL(file, directory), 'utf8');
      return requested.test(source) || expanded.test(source);
    })
    .sort();
  const documented = audit.entries.flatMap((entry) => {
    assert.ok(entry.classification);
    return entry.files;
  });
  assert.equal(new Set(documented).size, documented.length);
  assert.deepEqual(documented.sort(), matched);
});
