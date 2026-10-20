import { createHash } from 'node:crypto';
import biosPin from './pr-bios.json' with { type: 'json' };

export const biosShards = Object.freeze([...biosPin.shards]);
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');

export function biosShardContract(pin, bytes) {
  const incomplete = [];
  let catalog;
  try {
    if (!/^[a-f0-9]{40}$/.test(pin.commit ?? '') || pin.repository !== 'ai-ecoverse/slicc-bios')
      throw new Error('exact BIOS catalog source unavailable');
    if (
      !Array.isArray(pin.shards) ||
      !pin.shards.length ||
      new Set(pin.shards).size !== pin.shards.length ||
      pin.shards.some((name) => typeof name !== 'string' || !/^[a-z0-9][a-z0-9-]*$/.test(name))
    )
      throw new Error('unique explicit BIOS shard names required');
    if (!bytes || bytes.length > 64 * 1024)
      throw new Error('BIOS shard catalog absent or exceeds 64 KiB');
    catalog = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes));
    if (!catalog || typeof catalog !== 'object' || Array.isArray(catalog))
      throw new Error('BIOS shard catalog must be an object');
    for (const name of pin.shards) {
      const definition = catalog[name];
      if (!Object.hasOwn(catalog, name))
        incomplete.push(`unknown BIOS shard ${name} at ${pin.commit}`);
      else if (
        !definition ||
        typeof definition !== 'object' ||
        ![definition.match, definition.exclude].some((value) => typeof value === 'string' && value)
      )
        incomplete.push(`missing BIOS shard definition ${name} at ${pin.commit}`);
    }
  } catch (error) {
    incomplete.push(error.message);
  }
  return {
    schema: 1,
    status: incomplete.length ? 'INCOMPLETE' : 'PASS',
    repository: pin.repository,
    commit: pin.commit,
    path: 'scripts/canary/shards.json',
    shards: pin.shards,
    ...(bytes ? { sha256: digest(bytes) } : {}),
    incomplete,
  };
}

export async function fetchBiosShardContract(pin, fetcher = fetch) {
  try {
    if (!/^[a-f0-9]{40}$/.test(pin.commit ?? '') || pin.repository !== 'ai-ecoverse/slicc-bios')
      throw new Error('exact BIOS catalog source unavailable');
    const response = await fetcher(
      `https://raw.githubusercontent.com/${pin.repository}/${pin.commit}/scripts/canary/shards.json`,
      { signal: AbortSignal.timeout(30000) }
    );
    if (!response.ok)
      throw new Error(`BIOS shard catalog HTTP ${response.status} at ${pin.commit}`);
    const chunks = [];
    let size = 0;
    for await (const chunk of response.body) {
      size += chunk.length;
      if (size > 64 * 1024) throw new Error('BIOS shard catalog exceeds 64 KiB');
      chunks.push(chunk);
    }
    return biosShardContract(pin, Buffer.concat(chunks));
  } catch (error) {
    return {
      schema: 1,
      status: 'INCOMPLETE',
      repository: pin.repository,
      commit: pin.commit,
      path: 'scripts/canary/shards.json',
      shards: pin.shards,
      incomplete: [error.message],
    };
  }
}
