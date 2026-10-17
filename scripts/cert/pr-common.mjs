import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { matchesGlob } from 'node:path';
import { identity } from './common.mjs';

export const context = 'cert-review-pr';
export const residual =
  'self-guard catches accidental cert-path edits; not enforced against a deliberate rewrite of the workflow';
export const required = [
  'rebuild',
  'guard',
  'integration',
  'bios',
  'assets-linux',
  'assets-macos',
  'review',
];
export const prIdentity = (plan) => ({
  ...identity(plan),
  repository: plan.repository,
  pr: plan.pr,
  head: plan.head,
  base: plan.base,
  tools: plan.tools,
});
export const hash = (bytes, algorithm = 'sha256', encoding = 'hex') =>
  createHash(algorithm).update(bytes).digest(encoding);
export function client(
  repository,
  origin = process.env.GITHUB_API_URL ?? 'https://api.github.com'
) {
  if (!/^[\w.-]+\/[\w.-]+$/.test(repository)) throw new Error('invalid repository');
  const api = async (route, method = 'GET', body) => {
    const response = await fetch(`${origin}/repos/${repository}/${route}`, {
      method,
      headers: {
        Accept: 'application/vnd.github+json',
        'Content-Type': 'application/json',
        ...(process.env.GH_TOKEN ? { Authorization: `Bearer ${process.env.GH_TOKEN}` } : {}),
      },
      body: body ? JSON.stringify(body) : undefined,
      signal: AbortSignal.timeout(60000),
    });
    if (!response.ok) throw new Error(`GitHub ${method} ${route}: HTTP ${response.status}`);
    return response.status === 204 ? null : response.json();
  };
  const pages = async (route) => {
    const rows = [];
    for (let page = 1; page <= 30; page++) {
      const entries = await api(
        `${route}${route.includes('?') ? '&' : '?'}per_page=100&page=${page}`
      );
      if (!Array.isArray(entries)) throw new Error('invalid paginated response');
      rows.push(...entries);
      if (entries.length < 100) return rows;
    }
    throw new Error('GitHub evidence exceeds pagination limit');
  };
  return { api, pages };
}
export function reviewerPaths(paths) {
  const globs = JSON.parse(readFileSync(new URL('./pr-paths.json', import.meta.url), 'utf8'));
  return paths.some((file) => globs.some((glob) => matchesGlob(file, glob)));
}
