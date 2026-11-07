import { readdir, readFile, stat } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

const CLI_TIMEOUT_MS = 1500;
const CLI_MAX_STDOUT = 1_000_000;
const PROBE_TIMEOUT_MS = 500;
const PROFILE_NAME = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/u;
const FALLBACK_PORT = 9222;

function nonempty(value) {
  if (typeof value !== 'string') return undefined;
  const trimmed = value.trim();
  if (trimmed.length === 0) return undefined;
  return trimmed;
}

function isRecord(value) {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function loopbackHost(host) {
  const name = String(host ?? '')
    .toLowerCase()
    .replace(/^\[|\]$/gu, '');
  return name === '127.0.0.1' || name === 'localhost' || name === '::1';
}

export function acceptLoopbackWs(url, expectedPort) {
  let parsed;
  try {
    parsed = new URL(url);
  } catch {
    return false;
  }
  if (parsed.protocol !== 'ws:') return false;
  if (!loopbackHost(parsed.hostname)) return false;
  const port = parsed.port === '' ? 80 : Number(parsed.port);
  return Number.isInteger(port) && port === expectedPort;
}

function joinParts(parts) {
  return path.join(...parts);
}

function configDir(env, platform) {
  const home = nonempty(env.HOME) ?? os.homedir();
  if (platform === 'darwin') {
    return joinParts([home, 'Library', 'Application Support']);
  }
  if (platform === 'win32') {
    return nonempty(env.APPDATA) ?? joinParts([home, 'AppData', 'Roaming']);
  }
  const xdg = nonempty(env.XDG_CONFIG_HOME);
  if (xdg && path.isAbsolute(xdg)) return xdg;
  return joinParts([home, '.config']);
}

function sixProfilesRoot(env, platform) {
  const home = nonempty(env.HOME) ?? os.homedir();
  if (platform === 'darwin') {
    return joinParts([home, 'Library', 'Application Support', 'Slicc', 'profiles']);
  }
  if (platform === 'win32') {
    const local = nonempty(env.LOCALAPPDATA) ?? joinParts([home, 'AppData', 'Local']);
    return joinParts([local, 'Slicc', 'profiles']);
  }
  const xdg = nonempty(env.XDG_STATE_HOME);
  if (xdg && path.isAbsolute(xdg)) return joinParts([xdg, 'slicc', 'profiles']);
  return joinParts([home, '.local', 'state', 'slicc', 'profiles']);
}

export function sliccProfileRoots(options = {}) {
  const env = options.env ?? process.env;
  const platform = options.platform ?? process.platform;
  const config = configDir(env, platform);
  return [
    joinParts([config, 'slicc', 'seven-profiles']),
    joinParts([config, 'slicc', 'profiles']),
    sixProfilesRoot(env, platform),
  ];
}

export function isSliccUserDataDir(dir) {
  if (typeof dir !== 'string' || dir.length === 0) return false;
  const normalized = dir.replaceAll('\\', '/');
  return (
    normalized.includes('/slicc/seven-profiles/') ||
    normalized.includes('/slicc/profiles/') ||
    normalized.includes('/Slicc/profiles/') ||
    /\/slicc\/seven-profiles$/u.test(normalized) ||
    /\/slicc\/profiles$/u.test(normalized) ||
    /\/Slicc\/profiles$/u.test(normalized)
  );
}

function portFromCdp(cdp) {
  if (typeof cdp !== 'string') return undefined;
  try {
    const parsed = new URL(cdp);
    if (!loopbackHost(parsed.hostname)) return undefined;
    const port =
      parsed.port === '' ? (parsed.protocol === 'https:' ? 443 : 80) : Number(parsed.port);
    return Number.isInteger(port) ? port : undefined;
  } catch {
    return undefined;
  }
}

function endpointCandidates(entries) {
  if (!Array.isArray(entries)) return [];
  const out = [];
  for (const entry of entries) {
    if (!isRecord(entry) || entry.kind !== 'endpoint' || !isRecord(entry.endpoint)) continue;
    const { endpoint } = entry;
    const ws = endpoint.webSocketDebuggerUrl;
    const port = portFromCdp(endpoint.cdp);
    if (typeof ws !== 'string' || port === undefined) continue;
    if (!acceptLoopbackWs(ws, port)) continue;
    const tier = Number.isInteger(entry.tier) ? entry.tier : 99;
    out.push({
      url: ws,
      sevenOpen: endpoint.sevenOpen === true,
      sliccProfile:
        (typeof endpoint.profile === 'string' && endpoint.profile.length > 0) ||
        isSliccUserDataDir(endpoint.userDataDir),
      tier,
    });
  }
  return out;
}

function pickCliEndpoint(entries) {
  const candidates = endpointCandidates(entries);
  if (candidates.length === 0) return undefined;
  candidates.sort((left, right) => {
    if (left.sevenOpen !== right.sevenOpen) return left.sevenOpen ? -1 : 1;
    if (left.sliccProfile !== right.sliccProfile) return left.sliccProfile ? -1 : 1;
    return left.tier - right.tier;
  });
  return candidates[0].url;
}

export async function loadSpawn() {
  const mod = await import('node:child_process');
  return mod.spawn;
}

async function runSliccBrowsers(options) {
  let spawnImpl = options.spawn;
  if (spawnImpl === undefined) {
    try {
      spawnImpl = await (options.loadSpawn ?? loadSpawn)();
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      return { ok: false, reason: `child_process unavailable (${message})` };
    }
  }
  const timeoutMs = options.cliTimeoutMs ?? CLI_TIMEOUT_MS;
  const maxBytes = options.cliMaxStdout ?? CLI_MAX_STDOUT;
  const env = options.env ?? process.env;
  return new Promise((resolve) => {
    let child;
    try {
      child = spawnImpl('slicc', ['browsers', '--json'], {
        stdio: ['ignore', 'pipe', 'ignore'],
        env,
        shell: false,
        windowsHide: true,
      });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      resolve({ ok: false, reason: `spawn failed (${message})` });
      return;
    }
    let settled = false;
    let stdout = Buffer.alloc(0);
    let oversized = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(result);
    };
    const timer = setTimeout(() => {
      oversized = true;
      child.kill('SIGKILL');
      finish({ ok: false, reason: `timed out after ${timeoutMs}ms` });
    }, timeoutMs);
    child.on('error', (error) => {
      if (error && error.code === 'ENOENT') {
        finish({ ok: false, reason: 'slicc not on PATH' });
        return;
      }
      const message = error instanceof Error ? error.message : String(error);
      finish({ ok: false, reason: `spawn error (${message})` });
    });
    child.stdout.on('data', (chunk) => {
      if (settled || oversized) return;
      stdout = Buffer.concat([stdout, chunk]);
      if (stdout.length > maxBytes) {
        oversized = true;
        child.kill('SIGKILL');
        finish({ ok: false, reason: 'stdout exceeded size cap' });
      }
    });
    child.on('close', (code) => {
      if (settled) return;
      if (code !== 0) {
        finish({ ok: false, reason: `exited ${code ?? 'null'}` });
        return;
      }
      let value;
      try {
        value = JSON.parse(stdout.toString('utf8'));
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        finish({ ok: false, reason: `invalid JSON (${message})` });
        return;
      }
      if (!isRecord(value) || value.schema !== 1 || !Array.isArray(value.entries)) {
        finish({ ok: false, reason: 'schema 1 document missing or invalid' });
        return;
      }
      const url = pickCliEndpoint(value.entries);
      if (url === undefined) {
        finish({ ok: false, reason: 'no usable loopback endpoint' });
        return;
      }
      finish({ ok: true, url });
    });
  });
}

async function readActivePort(file) {
  const text = await readFile(file, 'utf8');
  const lines = text.split(/\r?\n/u);
  const port = Number(lines[0]?.trim());
  const wsPath = lines[1]?.trim();
  if (!Number.isInteger(port) || port < 1 || port > 65535) return undefined;
  if (typeof wsPath !== 'string' || !wsPath.startsWith('/')) return undefined;
  return { port, wsPath };
}

async function probeVersion(fetchImpl, port, timeoutMs) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetchImpl(`http://127.0.0.1:${port}/json/version`, {
      signal: controller.signal,
      redirect: 'manual',
      headers: { accept: 'application/json' },
    });
    if (!response.ok) return undefined;
    const body = await response.text();
    let value;
    try {
      value = JSON.parse(body);
    } catch {
      return undefined;
    }
    const url = isRecord(value) ? value.webSocketDebuggerUrl : undefined;
    if (typeof url !== 'string' || !acceptLoopbackWs(url, port)) return undefined;
    return url;
  } catch {
    return undefined;
  } finally {
    clearTimeout(timer);
  }
}

async function collectActivePorts(roots) {
  const found = [];
  for (const root of roots) {
    let entries;
    try {
      entries = await readdir(root, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      if (!entry.isDirectory()) continue;
      if (!PROFILE_NAME.test(entry.name)) continue;
      const file = path.join(root, entry.name, 'DevToolsActivePort');
      try {
        const active = await readActivePort(file);
        if (active === undefined) continue;
        const mtimeMs = (await stat(file)).mtimeMs;
        found.push({ ...active, file, mtimeMs });
      } catch {}
    }
  }
  return found;
}

async function fromDevToolsActivePort(options) {
  const fetchImpl = options.fetch ?? globalThis.fetch;
  const timeoutMs = options.probeTimeoutMs ?? PROBE_TIMEOUT_MS;
  const roots = options.profileRoots ?? sliccProfileRoots(options);
  const files = await collectActivePorts(roots);
  files.sort((left, right) => right.mtimeMs - left.mtimeMs);
  for (const item of files) {
    const url = await probeVersion(fetchImpl, item.port, timeoutMs);
    if (url === undefined) continue;
    if (new URL(url).pathname !== item.wsPath) continue;
    return { ok: true, url };
  }
  if (files.length === 0) {
    return { ok: false, reason: 'no DevToolsActivePort files under slicc profile dirs' };
  }
  return { ok: false, reason: 'no live DevToolsActivePort (stale or rejected websocket URL)' };
}

async function fromFallback(options) {
  const fetchImpl = options.fetch ?? globalThis.fetch;
  const timeoutMs = options.probeTimeoutMs ?? PROBE_TIMEOUT_MS;
  const url = await probeVersion(fetchImpl, FALLBACK_PORT, timeoutMs);
  if (url !== undefined) return { ok: true, url };
  return { ok: false, reason: 'no answer on /json/version' };
}

export async function resolveCdpEndpoint(options = {}) {
  const env = options.env ?? process.env;
  const tried = [];
  const explicit = nonempty(env.SLICC_CDP_URL);
  if (explicit !== undefined) {
    return { url: explicit, tried: ['SLICC_CDP_URL: set'] };
  }
  tried.push('SLICC_CDP_URL: not set');

  const cli = await runSliccBrowsers({ ...options, env });
  if (cli.ok) {
    tried.push('slicc browsers --json: chose a loopback endpoint');
    return { url: cli.url, tried };
  }
  tried.push(`slicc browsers --json: ${cli.reason}`);

  const active = await fromDevToolsActivePort({ ...options, env });
  if (active.ok) {
    tried.push('DevToolsActivePort: chose a live loopback port');
    return { url: active.url, tried };
  }
  tried.push(`DevToolsActivePort: ${active.reason}`);

  const fallback = await fromFallback(options);
  if (fallback.ok) {
    tried.push(`127.0.0.1:${FALLBACK_PORT}: answered`);
    return { url: fallback.url, tried };
  }
  tried.push(`127.0.0.1:${FALLBACK_PORT}: ${fallback.reason}`);
  throw new Error(`no debugger is connected\n${tried.map((line) => `- ${line}`).join('\n')}\n`);
}
