import { spawn } from 'node:child_process';
import path from 'node:path';
import { HARNESS_PORTS } from './ports.mjs';

export const harBinary =
  process.env.SLICC_HAR_BIN ||
  path.resolve(
    'target/debug',
    process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
  );

export function harHolder(endpoint, args, cwd) {
  const env = {
    ...process.env,
    HOME: cwd,
    USERPROFILE: cwd,
    XDG_CONFIG_HOME: cwd,
    XDG_STATE_HOME: cwd,
    APPDATA: cwd,
    SLICC_CDP_PORTS: HARNESS_PORTS,
    SLICC_FETCH_TRAIL: '1',
  };
  delete env.SLICC_CDP_URL;
  const child = spawn(harBinary, ['--cdp', endpoint, ...args], {
    cwd,
    env,
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  let stdout = '';
  let stderr = '';
  const waiting = [];
  child.stdout.on('data', (chunk) => {
    stdout += chunk;
    for (const wake of waiting.splice(0)) wake();
  });
  child.stderr.on('data', (chunk) => {
    stderr += chunk;
  });
  const timer = setTimeout(() => child.kill('SIGKILL'), 60000);
  let finished = false;
  const ended = new Promise((resolve) => {
    child.on('close', (code) => {
      finished = true;
      clearTimeout(timer);
      for (const wake of waiting.splice(0)) wake();
      resolve({ code, stdout, stderr });
    });
  });
  const events = () => stdout.split('\n').filter(Boolean).map(JSON.parse);
  const event = async (name, predicate = () => true) => {
    for (;;) {
      const found = events().find((item) => item.event === name && predicate(item));
      if (found) return found;
      if (finished) throw new Error(`holder exited before ${name}: ${stdout} ${stderr}`);
      await new Promise((resolve) => waiting.push(resolve));
    }
  };
  const output = () => ({ stdout, stderr });
  return { child, ended, event, events, output };
}

export async function harCommand(endpoint, args, cwd) {
  const holder = harHolder(endpoint, args, cwd);
  holder.child.stdin.end();
  return holder.ended;
}
