import { createHash } from 'node:crypto';

const ATTEMPTS = 4;
const BACKOFFS_MS = [1000, 2000, 4000];
const BACKOFF_CAP_MS = 15000;
const ASSET = 'slicc-extension.zip';
const API_TAG = 'https://api.github.com/repos/ai-ecoverse/slicc-extension/releases/tags';

const sleepDefault = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function authHeaders() {
  const token = process.env.GITHUB_TOKEN ?? process.env.GH_TOKEN;
  return token ? { Authorization: `Bearer ${token}` } : {};
}

function retryable(status) {
  return status === 429 || (status >= 500 && status <= 599);
}

function backoffMs(response, attempt) {
  const header = response?.headers?.get?.('retry-after');
  if (header) {
    const seconds = Number(header);
    if (Number.isFinite(seconds) && seconds >= 0) return Math.min(seconds * 1000, BACKOFF_CAP_MS);
    const when = Date.parse(header);
    if (!Number.isNaN(when)) return Math.min(Math.max(0, when - Date.now()), BACKOFF_CAP_MS);
  }
  return Math.min(BACKOFFS_MS[attempt] ?? BACKOFFS_MS.at(-1), BACKOFF_CAP_MS);
}

function checkSha(extension, zip) {
  const sum = createHash('sha256').update(zip).digest('hex');
  if (sum !== extension.sha256)
    throw new Error(`${extension.url}: sha256 ${sum}, want ${extension.sha256}`);
  return zip;
}

async function fetchBytes(url, init, kind, deps) {
  const doFetch = deps.fetch ?? globalThis.fetch;
  const sleep = deps.sleep ?? sleepDefault;
  let last;
  for (let attempt = 0; attempt < ATTEMPTS; attempt += 1) {
    let response;
    try {
      response = await doFetch(url, init);
    } catch (error) {
      last = new Error(`${kind} ${url}: ${error instanceof Error ? error.message : String(error)}`);
      if (attempt < ATTEMPTS - 1) await sleep(backoffMs(null, attempt));
      continue;
    }
    if (response.ok) return Buffer.from(await response.arrayBuffer());
    last = new Error(`${kind} ${url}: HTTP ${response.status}`);
    if (!retryable(response.status)) throw last;
    if (attempt < ATTEMPTS - 1) await sleep(backoffMs(response, attempt));
  }
  throw last;
}

async function fromApi(extension, deps) {
  const tagUrl = deps.tagUrl ?? `${API_TAG}/v${extension.version}`;
  const doFetch = deps.fetch ?? globalThis.fetch;
  const sleep = deps.sleep ?? sleepDefault;
  let release;
  let last;
  for (let attempt = 0; attempt < ATTEMPTS; attempt += 1) {
    try {
      release = await doFetch(tagUrl, {
        headers: { Accept: 'application/vnd.github+json', ...authHeaders() },
      });
    } catch (error) {
      last = new Error(
        `api tag ${tagUrl}: ${error instanceof Error ? error.message : String(error)}`
      );
      if (attempt < ATTEMPTS - 1) await sleep(backoffMs(null, attempt));
      continue;
    }
    if (release.ok) break;
    last = new Error(`api tag ${tagUrl}: HTTP ${release.status}`);
    if (!retryable(release.status)) throw last;
    if (attempt < ATTEMPTS - 1) await sleep(backoffMs(release, attempt));
    release = undefined;
  }
  if (!release?.ok) throw last;
  const body = await release.json();
  const asset = (body.assets ?? []).find((item) => item.name === ASSET);
  if (!asset?.url) throw new Error(`api tag ${tagUrl}: no ${ASSET} asset`);
  return fetchBytes(
    asset.url,
    {
      headers: { Accept: 'application/octet-stream', ...authHeaders() },
      redirect: 'follow',
    },
    'api asset',
    deps
  );
}

export async function fetchExtension(extension, deps = {}) {
  let zip;
  let releaseError;
  try {
    zip = await fetchBytes(extension.url, {}, 'releases/download', deps);
  } catch (error) {
    releaseError = error instanceof Error ? error : new Error(String(error));
  }
  if (zip) return checkSha(extension, zip);
  let apiError;
  try {
    zip = await fromApi(extension, deps);
  } catch (error) {
    apiError = error instanceof Error ? error : new Error(String(error));
    throw new Error(
      `slicc-extension v${extension.version}: releases/download failed (${releaseError.message}); api asset failed (${apiError.message})`
    );
  }
  return checkSha(extension, zip);
}
