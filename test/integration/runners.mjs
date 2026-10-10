import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { createNodeKernel } from '@ai-ecoverse/slicc-kernel/node';
import { bridge } from './browser.mjs';

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

export function runProcess(file, args, cwd, timeoutMs = 45000) {
  return new Promise((resolve, reject) => {
    const env = { ...process.env };
    delete env.SLICC_CDP_URL;
    const child = spawn(file, args, { cwd, env });
    const out = [];
    const err = [];
    child.stdout.on('data', (chunk) => out.push(chunk));
    child.stderr.on('data', (chunk) => err.push(chunk));
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
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
      const dir = path.join(cwd, '.playwright-cli');
      await mkdir(dir, { recursive: true });
      await writeFile(path.join(dir, 'session.json'), '{}\n');
    },
    async close() {},
  };
}

export async function kernelRunner(playwrightWasm, curlWasm, wsUrl) {
  const kernel = await createNodeKernel({ cdp: bridge(wsUrl) });
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
    playwright(args, cwd) {
      return kernelRun(kernel, ['playwright-cli', ...args], cwd);
    },
    curl(args, cwd) {
      return kernelRun(kernel, ['curlwright', ...args], cwd);
    },
    write(cwd, name, data) {
      return kernel.writeFile(`${cwd}/${name}`, data);
    },
    read(cwd, name) {
      return kernel.readFile(`${cwd}/${name}`);
    },
    clearSession(cwd) {
      return kernel.writeFile(`${cwd}/.playwright-cli/session.json`, '{}\n');
    },
    close() {
      kernel.terminate();
    },
  };
}

async function kernelRun(kernel, argv, cwd) {
  const result = await kernel.run(argv, { cwd });
  return { code: result.status, stdout: result.stdout, stderr: result.stderr };
}
