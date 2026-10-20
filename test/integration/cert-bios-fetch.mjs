import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';

const snapshot = JSON.parse(
  readFileSync(new URL('./cert-bios-shard-catalog.json', import.meta.url))
);
const bytes = Buffer.from(snapshot.content);
assert.equal(createHash('sha256').update(bytes).digest('hex'), snapshot.sha256);
const url = `https://raw.githubusercontent.com/${snapshot.repository}/${snapshot.commit}/scripts/canary/shards.json`;
const originalFetch = globalThis.fetch;

globalThis.fetch = (input, options) =>
  input === url ? Promise.resolve(new Response(bytes)) : originalFetch(input, options);
