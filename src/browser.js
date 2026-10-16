import { execFile as execFileCallback, spawn } from 'node:child_process';
import { createWriteStream, existsSync, statSync } from 'node:fs';
import { mkdir, mkdtemp, readdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { promisify } from 'node:util';

const execFile = promisify(execFileCallback);

export const CFT = 'https://googlechromelabs.github.io/chrome-for-testing';
export const TRUSTED_HOSTS = ['googlechromelabs.github.io', 'storage.googleapis.com'];
export const CHANNELS = ['Stable', 'Beta', 'Dev', 'Canary'];
export const MARKER = '.slicc-cdp-browser.json';
export const PROFILE_PREFIX = 'slicc-cdp-profile-';
export const RESERVED_PORTS = [9222, 9229];
const EXACT = /^\d+\.\d+\.\d+\.\d+$/u;
const MILESTONE = /^\d+$/u;

export function cftPlatform(platform = process.platform, arch = process.arch) {
  if (platform === 'darwin') return arch === 'arm64' ? 'mac-arm64' : 'mac-x64';
  if (platform === 'linux') {
    if (arch === 'x64') return 'linux64';
    if (arch === 'arm64') return 'linux-arm64';
    return null;
  }
  if (platform === 'win32') return arch === 'ia32' ? 'win32' : 'win64';
  return null;
}

export function executableIn(dir, platform) {
  const root = join(dir, `chrome-${platform}`);
  if (platform.startsWith('mac-')) {
    return join(
      root,
      'Google Chrome for Testing.app',
      'Contents',
      'MacOS',
      'Google Chrome for Testing'
    );
  }
  if (platform.startsWith('win')) return join(root, 'chrome.exe');
  return join(root, 'chrome');
}

export function browsersRoot(env = process.env, platform = process.platform, home = homedir()) {
  if (env.SLICC_BROWSERS_PATH) return env.SLICC_BROWSERS_PATH;
  if (platform === 'darwin') return join(home, 'Library', 'Caches', 'slicc-cdp', 'browsers');
  if (platform === 'win32') {
    return join(env.LOCALAPPDATA || join(home, 'AppData', 'Local'), 'slicc-cdp', 'browsers');
  }
  return join(env.XDG_CACHE_HOME || join(home, '.cache'), 'slicc-cdp', 'browsers');
}

export function systemCandidates(env = process.env, platform = process.platform) {
  if (platform === 'darwin') {
    return [
      'Google Chrome',
      'Google Chrome for Testing',
      'Chromium',
      'Google Chrome Canary',
      'Google Chrome Beta',
      'Google Chrome Dev',
    ].map((app) => `/Applications/${app}.app/Contents/MacOS/${app}`);
  }
  if (platform === 'win32') {
    const roots = [env.PROGRAMFILES, env['PROGRAMFILES(X86)'], env.LOCALAPPDATA].filter(Boolean);
    return roots.flatMap((root) => [
      join(root, 'Google', 'Chrome', 'Application', 'chrome.exe'),
      join(root, 'Chromium', 'Application', 'chrome.exe'),
    ]);
  }
  return [
    '/usr/bin/google-chrome',
    '/usr/bin/google-chrome-stable',
    '/usr/bin/chromium',
    '/usr/bin/chromium-browser',
    '/snap/bin/chromium',
  ];
}

function isFile(path) {
  try {
    return statSync(path).isFile();
  } catch {
    return false;
  }
}

export function compareVersions(a, b) {
  const left = a.split('.').map(Number);
  const right = b.split('.').map(Number);
  for (let i = 0; i < Math.max(left.length, right.length); i += 1) {
    const diff = (left[i] ?? 0) - (right[i] ?? 0);
    if (diff !== 0) return diff;
  }
  return 0;
}

export async function installedBrowsers({ root = browsersRoot(), platform = cftPlatform() } = {}) {
  const names = await readdir(root).catch(() => []);
  const found = [];
  for (const name of names) {
    const marker = await readFile(join(root, name, MARKER), 'utf8').catch(() => null);
    if (marker === null) continue;
    let info;
    try {
      info = JSON.parse(marker);
    } catch {
      continue;
    }
    if (info.platform !== platform || typeof info.version !== 'string') continue;
    const executable = executableIn(join(root, name), platform);
    if (!isFile(executable)) continue;
    found.push({ version: info.version, platform, dir: join(root, name), executable });
  }
  return found.sort((a, b) => compareVersions(b.version, a.version));
}

export async function findBrowser({
  env = process.env,
  platform = process.platform,
  arch = process.arch,
  root = browsersRoot(env, platform),
  candidates = systemCandidates(env, platform),
  version,
} = {}) {
  if (env.CHROME_PATH && !version) {
    if (isFile(env.CHROME_PATH)) return { executable: env.CHROME_PATH, source: 'env' };
    throw new Error(`CHROME_PATH is ${env.CHROME_PATH}, which is not a file`);
  }
  const target = cftPlatform(platform, arch);
  if (target) {
    const installs = await installedBrowsers({ root, platform: target });
    const match = version
      ? installs.find((item) => item.version === version || item.version.startsWith(`${version}.`))
      : installs[0];
    if (match) return { executable: match.executable, source: 'installed', version: match.version };
    if (version) return null;
  }
  const system = candidates.find(isFile);
  if (system) return { executable: system, source: 'system' };
  return null;
}

export function trusted(url) {
  let parsed;
  try {
    parsed = new URL(url);
  } catch {
    return false;
  }
  return parsed.protocol === 'https:' && TRUSTED_HOSTS.includes(parsed.hostname);
}

function channelName(channel) {
  const match = CHANNELS.find((name) => name.toLowerCase() === String(channel).toLowerCase());
  if (!match) throw new Error(`unknown channel "${channel}" (expected ${CHANNELS.join(', ')})`);
  return match;
}

async function json(fetchImpl, url) {
  const response = await fetchImpl(url);
  if (!response.ok) throw new Error(`GET ${url}: ${response.status} ${response.statusText}`);
  return response.json();
}

export async function resolveRelease({
  channel = 'Stable',
  version,
  fetch: fetchImpl = fetch,
} = {}) {
  let entry;
  if (version && EXACT.test(version)) {
    const data = await json(fetchImpl, `${CFT}/known-good-versions-with-downloads.json`);
    entry = data.versions?.find((item) => item.version === version);
  } else if (version && MILESTONE.test(version)) {
    const data = await json(fetchImpl, `${CFT}/latest-versions-per-milestone-with-downloads.json`);
    entry = data.milestones?.[version];
  } else if (version) {
    throw new Error(
      `unsupported version "${version}" (expected a full version such as 148.0.7778.97, or a milestone)`
    );
  } else {
    const data = await json(fetchImpl, `${CFT}/last-known-good-versions-with-downloads.json`);
    entry = data.channels?.[channelName(channel)];
  }
  if (!entry) throw new Error(`Chrome for Testing has no release for ${version ?? channel}`);
  return { version: entry.version, downloads: entry.downloads?.chrome ?? [] };
}

async function download(fetchImpl, url, file) {
  if (!trusted(url)) {
    throw new Error(`refusing to download ${url}: only https from ${TRUSTED_HOSTS.join(', ')}`);
  }
  const response = await fetchImpl(url, { redirect: 'error' });
  if (!response.ok || !response.body) {
    throw new Error(`GET ${url}: ${response.status} ${response.statusText}`);
  }
  await pipeline(Readable.fromWeb(response.body), createWriteStream(file));
  const expected = Number(response.headers.get('content-length'));
  const actual = statSync(file).size;
  if (expected > 0 && actual !== expected) {
    throw new Error(`GET ${url}: got ${actual} bytes, expected ${expected}`);
  }
}

export async function extract(zip, dir, platform, run = execFile) {
  if (platform.startsWith('mac')) return run('ditto', ['-x', '-k', zip, dir]);
  if (platform.startsWith('win')) return run('tar', ['-xf', zip, '-C', dir]);
  return run('unzip', ['-q', '-o', zip, '-d', dir]);
}

export async function verify(executable, version, platform, run = execFile) {
  if (!isFile(executable)) throw new Error(`${executable} is missing after extraction`);
  if (platform.startsWith('win')) {
    const manifest = join(executable, '..', `${version}.manifest`);
    if (!isFile(manifest)) throw new Error(`${manifest} is missing, so this is not ${version}`);
    return;
  }
  const { stdout } = await run(executable, ['--version']);
  if (!stdout.includes(version)) {
    throw new Error(`${executable} --version printed "${stdout.trim()}", expected ${version}`);
  }
}

export async function privilege({ run = execFile, uid = process.getuid?.() } = {}) {
  if (uid === 0) return [];
  const sudo = await run('sudo', ['-n', 'true']).then(
    () => true,
    () => false
  );
  if (sudo) return ['sudo', '-n'];
  throw new Error(
    '--with-deps installs system packages with apt-get and needs root or passwordless sudo'
  );
}

export async function installDeps(
  dir,
  platform,
  { run = execFile, uid = process.getuid?.() } = {}
) {
  if (!platform.startsWith('linux')) return;
  const deps = await readFile(join(dir, `chrome-${platform}`, 'deb.deps'), 'utf8');
  const list = deps
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean);
  if (list.length === 0) return;
  const [file, ...prefix] = [...(await privilege({ run, uid })), 'apt-get'];
  const env = { ...process.env, DEBIAN_FRONTEND: 'noninteractive' };
  await run(file, [...prefix, 'update'], { env });
  await run(file, [...prefix, 'satisfy', '-y', '--no-install-recommends', list.join(', ')], {
    env,
  });
}

export async function installBrowser({
  channel = 'Stable',
  version,
  root = browsersRoot(),
  platform = cftPlatform(),
  withDeps = false,
  force = false,
  fetch: fetchImpl = fetch,
  run = execFile,
  uid = process.getuid?.(),
  log = () => {},
} = {}) {
  if (!platform) {
    throw new Error(`Chrome for Testing has no build for ${process.platform} ${process.arch}`);
  }
  const deps = (dir) => (withDeps ? installDeps(dir, platform, { run, uid }) : undefined);
  if (version && EXACT.test(version) && !force) {
    const installs = await installedBrowsers({ root, platform });
    const ready = installs.find((item) => item.version === version);
    if (ready) {
      await deps(ready.dir);
      return { ...ready, installed: false };
    }
  }
  const release = await resolveRelease({ channel, version, fetch: fetchImpl });
  const dir = join(root, `chrome-${release.version}-${platform}`);
  const executable = executableIn(dir, platform);
  if (!force && existsSync(join(dir, MARKER)) && isFile(executable)) {
    await deps(dir);
    return { version: release.version, platform, dir, executable, installed: false };
  }
  const url = release.downloads.find((item) => item.platform === platform)?.url;
  if (!url) throw new Error(`Chrome for Testing ${release.version} has no ${platform} build`);
  await mkdir(root, { recursive: true });
  const staging = await mkdtemp(join(root, '.staging-'));
  try {
    const zip = join(staging, 'chrome.zip');
    log(`downloading Chrome for Testing ${release.version} (${platform}) from ${url}`);
    await download(fetchImpl, url, zip);
    const unpacked = join(staging, 'unpacked');
    await mkdir(unpacked);
    await extract(zip, unpacked, platform, run);
    await deps(unpacked);
    await verify(executableIn(unpacked, platform), release.version, platform, run);
    await writeFile(
      join(unpacked, MARKER),
      `${JSON.stringify({ version: release.version, platform, url }, null, 2)}\n`
    );
    await rm(dir, { recursive: true, force: true });
    await rename(unpacked, dir);
  } finally {
    await rm(staging, { recursive: true, force: true });
  }
  log(`installed ${executable}`);
  return { version: release.version, platform, dir, executable, installed: true };
}

export const LAUNCH_FLAGS = [
  '--no-first-run',
  '--no-default-browser-check',
  '--disable-background-networking',
  '--disable-component-update',
  '--disable-sync',
  '--password-store=basic',
  '--use-mock-keychain',
];

export function launchArgs({ userDataDir, port = 0, headless = true, sandbox = true, args = [] }) {
  return [
    `--remote-debugging-port=${port}`,
    '--remote-debugging-address=127.0.0.1',
    `--user-data-dir=${userDataDir}`,
    ...(headless ? ['--headless'] : []),
    ...(sandbox ? [] : ['--no-sandbox']),
    ...LAUNCH_FLAGS,
    ...args,
    'about:blank',
  ];
}

export const OWNED_SWITCHES = [
  '--remote-debugging-port',
  '--remote-debugging-address',
  '--remote-debugging-pipe',
  '--user-data-dir',
  '--profile-directory',
];

function switchName(arg, platform = process.platform) {
  const text = String(arg);
  const prefix =
    /^--?/.exec(text)?.[0] ?? (platform === 'win32' && text.startsWith('/') ? '/' : '');
  if (!prefix) return null;
  return text.slice(prefix.length).split('=')[0].toLowerCase();
}

function owned(arg, platform) {
  const name = switchName(arg, platform);
  return name !== null && OWNED_SWITCHES.some((owned) => owned.slice(2) === name);
}

const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const reserved = (port) => port >= RESERVED_PORTS[0] && port <= RESERVED_PORTS[1];
const gone = (child) => child.exitCode !== null || child.signalCode !== null;

async function versionAt(port, fetchImpl) {
  try {
    const response = await fetchImpl(`http://127.0.0.1:${port}/json/version`);
    if (!response.ok) return null;
    const { webSocketDebuggerUrl } = await response.json();
    if (typeof webSocketDebuggerUrl !== 'string') return null;
    return { port, path: new URL(webSocketDebuggerUrl).pathname };
  } catch {
    return null;
  }
}

async function ready(userDataDir, port, child, deadline, fetchImpl) {
  const file = join(userDataDir, 'DevToolsActivePort');
  while (Date.now() < deadline) {
    if (port === 0) {
      const text = await readFile(file, 'utf8').catch(() => '');
      const [found, path] = text.split('\n');
      if (found && path) return { port: Number(found), path: path.trim() };
    } else {
      const found = await versionAt(port, fetchImpl);
      if (found) return found;
    }
    if (gone(child)) return null;
    await pause(50);
  }
  return null;
}

export async function launchBrowser({
  executable,
  userDataDir,
  port = 0,
  headless = true,
  sandbox = true,
  args = [],
  env = process.env,
  timeout = 30000,
  fetch: fetchImpl = fetch,
  platform = process.platform,
} = {}) {
  const path = executable ?? (await findBrowser({ env }))?.executable;
  if (!path) {
    throw new Error(
      'no Chrome or Chromium found: set CHROME_PATH or run `slicc-cdp install-browser`'
    );
  }
  if (reserved(port)) {
    throw new Error(`refusing to bind port ${port} (9222-9229 belong to the kernel and the probe)`);
  }
  const taken = args.find((arg) => owned(arg, platform));
  if (taken) {
    throw new Error(`args may not set ${taken}: use the port and userDataDir options`);
  }
  if (port !== 0 && (await versionAt(port, fetchImpl))) {
    throw new Error(`a browser already listens on 127.0.0.1:${port}`);
  }
  const temporary = userDataDir === undefined;
  const profile = userDataDir ?? (await mkdtemp(join(tmpdir(), PROFILE_PREFIX)));
  await rm(join(profile, 'DevToolsActivePort'), { force: true });
  let log = '';
  const child = spawn(path, launchArgs({ userDataDir: profile, port, headless, sandbox, args }), {
    env,
    stdio: ['ignore', 'ignore', 'pipe'],
  });
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (chunk) => {
    log = (log + chunk).slice(-4000);
  });
  const spawned = new Promise((resolve) => child.once('error', resolve));
  const exited = new Promise((resolve) => child.once('close', resolve));
  const close = async () => {
    if (child.pid !== undefined && !gone(child)) {
      child.kill('SIGKILL');
      await exited;
    }
    if (temporary) await rm(profile, { recursive: true, force: true });
  };
  const found = await Promise.race([
    ready(profile, port, child, Date.now() + timeout, fetchImpl),
    spawned.then((error) => ({ error })),
  ]);
  if (!found || found.error) {
    const status = gone(child)
      ? `exited with ${child.signalCode ?? `code ${child.exitCode}`}`
      : `no DevTools endpoint after ${timeout} ms`;
    await close();
    const reason = found?.error?.message ?? (log.trim() || status);
    throw new Error(`${path} did not start: ${reason}`);
  }
  if (reserved(found.port)) {
    await close();
    throw new Error(`browser bound reserved port ${found.port}`);
  }
  return {
    child,
    executable: path,
    userDataDir: profile,
    port: found.port,
    wsUrl: `ws://127.0.0.1:${found.port}${found.path}`,
    close,
  };
}
