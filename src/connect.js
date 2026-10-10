import { CDPClient } from './cdp-client.js';

const UNRESERVED = /[A-Za-z0-9._~-]/u;

function nonempty(value) {
  if (typeof value !== 'string') return undefined;
  const trimmed = value.trim();
  if (trimmed.length === 0) return undefined;
  return trimmed;
}

export function chooseStart(cdp, env) {
  const explicit = nonempty(cdp);
  if (explicit !== undefined) return classify(explicit);
  const fromEnv = nonempty(env);
  if (fromEnv !== undefined) return classify(fromEnv);
  return { kind: 'discover', url: 'http://127.0.0.1:9222/json/version' };
}

function classify(url) {
  const lower = url.toLowerCase();
  if (lower.startsWith('ws://') || lower.startsWith('wss://')) {
    return { kind: 'direct', url };
  }
  if (lower.startsWith('http://') || lower.startsWith('https://')) {
    return { kind: 'discover', url: discoveryUrl(url) };
  }
  throw new Error(`unsupported CDP URL "${url}" (expected ws://, wss://, http://, or https://)\n`);
}

export function discoveryUrl(httpUrl) {
  if (httpUrl.includes('/json/version')) return httpUrl;
  const trimmed = httpUrl.replace(/\/+$/u, '');
  return `${trimmed}/json/version`;
}

export function appendRuntime(url, runtime) {
  const value = nonempty(runtime);
  if (value === undefined) return url;
  const enc = percentEncode(value);
  if (url.includes('?')) return `${url}&runtime=${enc}`;
  return `${url}?runtime=${enc}`;
}

function percentEncode(value) {
  let out = '';
  for (const byte of new TextEncoder().encode(value)) {
    const char = String.fromCharCode(byte);
    if (UNRESERVED.test(char)) out += char;
    else out += `%${byte.toString(16).toUpperCase().padStart(2, '0')}`;
  }
  return out;
}

export function formatHttpError(status, body) {
  const text = String(body ?? '').replace(/\0+$/u, '');
  if (text.length === 0) return `HTTP ${status}\n`;
  if (text.endsWith('\n')) return `HTTP ${status}\n${text}`;
  return `HTTP ${status}\n${text}\n`;
}

export function websocketUrlFromVersion(body) {
  let value;
  try {
    value = JSON.parse(body);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    throw new Error(`json/version: ${message}\n`);
  }
  const url = isRecord(value) ? value.webSocketDebuggerUrl : undefined;
  if (typeof url !== 'string' || url.length === 0) {
    throw new Error('json/version returned no webSocketDebuggerUrl\n');
  }
  return url;
}

function isRecord(value) {
  return Boolean(value) && typeof value === 'object';
}

export async function connect(options = {}) {
  const env = options.env ?? process.env.SLICC_CDP_URL;
  const start = chooseStart(options.url, env);
  const fetchImpl = options.fetch ?? globalThis.fetch;
  const socketUrl = await socketFromStart(start, fetchImpl);
  const withRuntime = appendRuntime(socketUrl, options.runtime);
  const client = new CDPClient(options.WebSocket ?? globalThis.WebSocket);
  await client.open(withRuntime, { timeout: options.timeout ?? 10000 });
  return client;
}

async function socketFromStart(start, fetchImpl) {
  if (start.kind === 'direct') return start.url;
  return discover(fetchImpl, start.url);
}

async function discover(fetchImpl, url) {
  let response;
  try {
    response = await fetchImpl(url);
  } catch (err) {
    throw discoveryFailure(err);
  }
  const body = await response.text();
  if (!response.ok) throw new Error(formatHttpError(response.status, body));
  return websocketUrlFromVersion(body);
}

function discoveryFailure(err) {
  const message = err instanceof Error ? err.message : String(err);
  if (message.startsWith('HTTP ') || message.includes('does not launch Chrome')) {
    return err instanceof Error ? err : new Error(message);
  }
  return new Error(
    `${message}\nThis CLI does not launch Chrome. Pass --cdp or set SLICC_CDP_URL.\n`
  );
}
