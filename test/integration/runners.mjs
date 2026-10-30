import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readdir, readFile, realpath, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { attachKernel, createNodeKernel } from '@ai-ecoverse/slicc-kernel/node';
import { bridge } from './browser.mjs';
import { requireChrome } from './chrome-policy.mjs';
import { deadline, killTree } from './watchdog.mjs';

const manifest = JSON.stringify({
  name: '@ai-ecoverse/slicc-cdp',
  slicc: {
    abi: 'wasi',
    commands: {
      'playwright-cli': { wasm: 'bin/playwright-cli.wasm' },
      curlwright: { wasm: 'bin/curlwright.wasm' },
    },
  },
});

function hostHome() {
  if (process.platform === 'win32' && process.env.USERPROFILE) return process.env.USERPROFILE;
  if (process.env.HOME) return process.env.HOME;
  return process.env.USERPROFILE || '';
}

function hostConfigDir() {
  const home = hostHome();
  if (process.platform === 'darwin') {
    return path.join(home, 'Library', 'Application Support');
  }
  if (process.platform === 'win32') {
    if (process.env.APPDATA) return process.env.APPDATA;
    return path.join(home, 'AppData', 'Roaming');
  }
  const xdg = process.env.XDG_CONFIG_HOME;
  if (xdg?.startsWith('/')) return xdg;
  return path.join(home, '.config');
}

async function hostSessionFile(cwd) {
  const canonical = await realpath(cwd);
  const key = createHash('sha256').update(canonical).digest('hex');
  return path.join(hostConfigDir(), 'slicc', 'playwright-cli', 'sessions', `${key}.json`);
}

export function runProcess(file, args, cwd, timeoutMs = 45000) {
  const explicitCdp = args.some((arg) => arg === '--cdp' || arg?.startsWith('--cdp='));
  if (/chrome|chromium/i.test(path.basename(file)) || (args.includes('open') && !explicitCdp))
    requireChrome();
  return new Promise((resolve, reject) => {
    const env = { ...process.env };
    delete env.SLICC_CDP_URL;
    const child = spawn(file, args, { cwd, env, detached: process.platform !== 'win32' });
    const out = [];
    const err = [];
    child.stdout.on('data', (chunk) => out.push(chunk));
    child.stderr.on('data', (chunk) => err.push(chunk));
    const timer = setTimeout(() => {
      killTree(child);
      reject(new Error(`timed out: ${path.basename(file)} ${args.join(' ')}`));
    }, timeoutMs);
    child.on('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on('close', (code) => {
      clearTimeout(timer);
      resolve({
        code: code ?? 1,
        stdout: Buffer.concat(out).toString('utf8'),
        stderr: Buffer.concat(err).toString('utf8'),
      });
    });
  });
}

export function nativeRunner(playwright, curlwright, wsUrl) {
  if (!wsUrl) requireChrome();
  return {
    async fresh() {
      return mkdtemp(path.join(tmpdir(), 'slicc-cdp-cli-'));
    },
    playwright(args, cwd) {
      return runProcess(playwright, ['--cdp', wsUrl, ...args], cwd);
    },
    curl(args, cwd) {
      return runProcess(curlwright, ['--cdp', wsUrl, ...args], cwd);
    },
    async write(cwd, name, data) {
      const file = path.join(cwd, name);
      await mkdir(path.dirname(file), { recursive: true });
      await writeFile(file, data);
    },
    read(cwd, name) {
      return readFile(path.join(cwd, name));
    },
    async clearSession(cwd) {
      const file = await hostSessionFile(cwd);
      await mkdir(path.dirname(file), { recursive: true });
      await writeFile(file, '{}\n');
    },
    async close() {},
  };
}

export async function kernelRunner(playwrightWasm, curlWasm, wsUrl, options = {}) {
  if (!wsUrl) requireChrome();
  const kernel = await createNodeKernel({ cdp: bridge(wsUrl), ...options });
  const pkg = '/node_modules/@ai-ecoverse/slicc-cdp';
  await kernel.writeFile(`${pkg}/package.json`, manifest);
  await kernel.writeFile(`${pkg}/bin/playwright-cli.wasm`, await readFile(playwrightWasm));
  await kernel.writeFile(`${pkg}/bin/curlwright.wasm`, await readFile(curlWasm));
  let next = 0;
  return {
    async fresh() {
      next += 1;
      const dir = `/home/case${next}`;
      await kernel.writeFile(`${dir}/.keep`, '');
      return dir;
    },
    playwright(args, cwd, options = {}) {
      return kernelRun(kernel, ['playwright-cli', ...args], cwd, options);
    },
    curl(args, cwd) {
      return kernelRun(kernel, ['curlwright', ...args], cwd);
    },
    run(argv, cwd, options = {}) {
      return kernelRun(kernel, argv, cwd, options);
    },
    addUser(name) {
      return kernel.users.add({ name });
    },
    writeFile(file, data) {
      return kernel.writeFile(file, data);
    },
    readFile(file) {
      return kernel.readFile(file);
    },
    async stat(file) {
      const client = await attachKernel(await kernel.connect());
      try {
        return await client.fs.stat(file);
      } finally {
        await client.close();
      }
    },
    async install(name) {
      const root = path.resolve('node_modules', name);
      for (const entry of await readdir(root, { recursive: true, withFileTypes: true })) {
        if (!entry.isFile()) continue;
        const file = path.join(entry.parentPath, entry.name);
        await kernel.writeFile(
          `/node_modules/${name}/${path.relative(root, file).split(path.sep).join('/')}`,
          await readFile(file)
        );
      }
    },
    async processes() {
      const client = await attachKernel(await kernel.connect());
      try {
        return await client.ps();
      } finally {
        await client.close();
      }
    },
    write(cwd, name, data) {
      return kernel.writeFile(`${cwd}/${name}`, data);
    },
    read(cwd, name) {
      return kernel.readFile(`${cwd}/${name}`);
    },
    clearSession(cwd) {
      return kernel.writeFile(kernelSessionFile('/root', cwd), '{}\n');
    },
    close() {
      kernel.terminate();
    },
  };
}

export function kernelSessionFile(home, cwd) {
  const key = createHash('sha256').update(cwd).digest('hex');
  return `${home}/.config/slicc/playwright-cli/sessions/${key}.json`;
}

async function kernelRun(kernel, argv, cwd, options = {}) {
  const result = await deadline(
    `kernel ${argv.join(' ')}`,
    300000,
    kernel.run(argv, { cwd, ...options })
  );
  return { code: result.status, stdout: result.stdout, stderr: result.stderr };
}
