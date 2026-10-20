import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';

const catalogs = new Map(
  ['cert-bios-shard-catalog.json', 'cert-bios-bound-shard-catalog.json'].map((file) => {
    const snapshot = JSON.parse(readFileSync(new URL(file, import.meta.url)));
    const bytes = Buffer.from(snapshot.content);
    assert.equal(createHash('sha256').update(bytes).digest('hex'), snapshot.sha256);
    assert.equal(
      createHash('sha1').update(`blob ${bytes.length}\0`).update(bytes).digest('hex'),
      snapshot.blob
    );
    return [
      `https://raw.githubusercontent.com/${snapshot.repository}/${snapshot.commit}/scripts/canary/shards.json`,
      bytes,
    ];
  })
);
const originalFetch = globalThis.fetch;

globalThis.fetch = (input, options) =>
  catalogs.has(input)
    ? Promise.resolve(new Response(catalogs.get(input)))
    : originalFetch(input, options);
