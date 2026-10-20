import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { isDeepStrictEqual } from 'node:util';
import { artifactDigest, artifactId, artifactInventory } from './pr-artifact-binding.mjs';

const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
const artifactName = (side, shard) => `${side}-canary-shard-canary-${shard}`;
const artifactUrl = (plan, id) =>
  `https://github.com/${plan.repository}/actions/runs/${plan.run}/artifacts/${id}`;
const uploadMatches = (item, plan, id, digest) =>
  artifactId(item?.id) === id &&
  artifactDigest(item?.digest) === digest &&
  !item.expired &&
  String(item.workflow_run?.id) === String(plan.run) &&
  item.workflow_run?.head_sha === plan.head;

function checkArtifact(plan, side, shard, artifact, inventory, ids, failures, incomplete) {
  const name = artifactName(side, shard);
  const id = artifactId(artifact?.id);
  const digest = artifactDigest(artifact?.digest);
  if (
    !id ||
    !digest ||
    ids.has(id) ||
    artifact.name !== name ||
    artifact.url !== artifactUrl(plan, id) ||
    artifact.zipSha256 !== digest
  )
    incomplete.push(`bios: ${side} actual shard artifact identity incomplete: ${shard}`);
  if (id) ids.add(id);
  if (!inventory) return;
  const found = inventory.filter((item) => item.name === name);
  if (found.length !== 1 || !uploadMatches(found[0], plan, id, digest))
    failures.push(`bios: ${side} actual shard upload binding differs: ${shard}`);
}

function packMatches(plan, side, document, subject) {
  const overrides = document?.pins?.overrides;
  const override = Array.isArray(overrides)
    ? overrides.find((item) => item?.name === '@ai-ecoverse/slicc-cdp')
    : undefined;
  return (
    Array.isArray(overrides) &&
    overrides.length === 1 &&
    override?.version === subject?.manifest.version &&
    override?.sha256 === subject?.sha256 &&
    override?.via === 'artifact' &&
    override.source?.artifact === `cert-pr-pack-${plan.pr}-${plan.head}-${plan.attempt}-${side}` &&
    String(override.source?.run) === String(plan.run) &&
    override.source?.repo === plan.repository
  );
}

function checkDocument(plan, side, shard, value, subject, actualSource, failures, incomplete) {
  try {
    if (!actualSource)
      incomplete.push(`bios: ${side} independently downloaded actual row source absent: ${shard}`);
    else if (!isDeepStrictEqual(value, actualSource))
      failures.push(`bios: ${side} actual row receipt differs from downloaded source: ${shard}`);
    if (hash(JSON.stringify(value.document)) !== value.artifact.rowsSha256)
      failures.push(`bios: ${side} actual shard row projection differs: ${shard}`);
    if (!packMatches(plan, side, value.document, subject))
      failures.push(`bios: ${side} actual shard pack/source differs: ${shard}`);
  } catch {
    incomplete.push(`bios: ${side} actual shard bytes or pins unavailable: ${shard}`);
  }
}

export function biosExecutionRowFindings(plan, pin, executions, subjects, inventory, actualSource) {
  const failures = [];
  const incomplete = [];
  if (executions === undefined) return { failures, incomplete };
  const ids = new Set();
  for (const side of ['base', 'candidate']) {
    if (
      !executions?.[side] ||
      Object.keys(executions[side]).some((shard) => !pin.shards.includes(shard))
    )
      incomplete.push(`bios: ${side} actual declared shards incomplete`);
    for (const shard of pin.shards) {
      const value = executions?.[side]?.[shard];
      checkArtifact(plan, side, shard, value?.artifact, inventory, ids, failures, incomplete);
      checkDocument(
        plan,
        side,
        shard,
        value,
        subjects[side],
        actualSource?.[side]?.[shard],
        failures,
        incomplete
      );
    }
  }
  return { failures, incomplete };
}

function selectArtifact(plan, inventory, side, shard) {
  const name = artifactName(side, shard);
  const matches = inventory.filter((item) => item.name === name);
  if (matches.length !== 1) throw new Error(`unique exact BIOS row artifact required: ${name}`);
  const id = artifactId(matches[0].id);
  const digest = artifactDigest(matches[0].digest);
  if (!id || !digest || !uploadMatches(matches[0], plan, id, digest))
    throw new Error(`BIOS row artifact identity incomplete: ${name}`);
  return { id, digest, name };
}

async function archiveBytes(plan, artifact) {
  const origin = process.env.GITHUB_API_URL ?? 'https://api.github.com';
  const response = await fetch(
    `${origin}/repos/${plan.repository}/actions/artifacts/${artifact.id}/zip`,
    {
      headers: process.env.GH_TOKEN ? { Authorization: `Bearer ${process.env.GH_TOKEN}` } : {},
      signal: AbortSignal.timeout(60000),
    }
  );
  if (!response.ok) throw new Error(`BIOS row archive HTTP ${response.status}: ${artifact.name}`);
  const chunks = [];
  let length = 0;
  for await (const chunk of response.body) {
    length += chunk.length;
    if (length > 32 * 1024 * 1024) throw new Error('BIOS row archive exceeds bound');
    chunks.push(chunk);
  }
  const zip = Buffer.concat(chunks);
  if (hash(zip) !== artifact.digest)
    throw Object.assign(new Error(`BIOS row ZIP digest mismatch: ${artifact.name}`), {
      certStatus: 'FAIL',
    });
  return zip;
}

function documentBytes(zip, file, name) {
  writeFileSync(file, zip, { flag: 'wx', mode: 0o600 });
  const names = execFileSync('unzip', ['-Z1', file], {
    encoding: 'utf8',
    maxBuffer: 1024 * 1024,
    timeout: 10000,
  })
    .trim()
    .split('\n');
  const entries = names.filter(
    (entry) =>
      !entry.startsWith('/') &&
      !entry.split('/').includes('..') &&
      (entry === 'shard.json' || entry.endsWith('/shard.json'))
  );
  if (entries.length !== 1)
    throw new Error(`canonical BIOS row document absent or ambiguous: ${name}`);
  return execFileSync('unzip', ['-p', file, entries[0]], {
    maxBuffer: 16 * 1024 * 1024,
    timeout: 10000,
  });
}

function projectDocument(bytes) {
  const raw = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes));
  return {
    shard: raw.shard,
    repeat: raw.repeat,
    exit: raw.exit,
    ...(raw.signal ? { signal: raw.signal } : {}),
    ...(raw.timedOut ? { timedOut: raw.timedOut } : {}),
    pins: { overrides: raw.pins?.overrides },
    results: raw.results?.map((row) => ({ key: row?.key, status: row?.status, runs: row?.runs })),
  };
}

export async function biosExecutionRows(plan, pin, api) {
  const inventory = await artifactInventory(api, plan.run);
  const rows = {};
  const directory = mkdtempSync(path.join(tmpdir(), 'slicc-bios-rows-'));
  try {
    for (const side of ['base', 'candidate']) {
      rows[side] = {};
      for (const shard of pin.shards) {
        const artifact = selectArtifact(plan, inventory, side, shard);
        const zip = await archiveBytes(plan, artifact);
        const bytes = documentBytes(
          zip,
          path.join(directory, `${side}-${shard}.zip`),
          artifact.name
        );
        const document = projectDocument(bytes);
        rows[side][shard] = {
          artifact: {
            ...artifact,
            url: artifactUrl(plan, artifact.id),
            zipSha256: hash(zip),
            documentSha256: hash(bytes),
            rowsSha256: hash(JSON.stringify(document)),
          },
          document,
        };
      }
    }
    return rows;
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}
