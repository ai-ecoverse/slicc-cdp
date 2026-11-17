import { spawn } from 'node:child_process';
import path from 'node:path';
import { HARNESS_PORTS } from './ports.mjs';

export const evalBinary =
  process.env.SLICC_EVAL_BIN ||
  path.resolve(
    'target/debug',
    process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
  );

export function evalHolder(endpoint, args, cwd) {
  const env = {
    ...process.env,
    HOME: cwd,
    USERPROFILE: cwd,
    XDG_CONFIG_HOME: cwd,
    XDG_STATE_HOME: cwd,
    APPDATA: cwd,
    SLICC_CDP_PORTS: HARNESS_PORTS,
  };
  delete env.SLICC_CDP_URL;
  const child = spawn(evalBinary, ['--cdp', endpoint, ...args], {
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
  const events = () => stdout.split('\n').slice(0, -1).filter(Boolean).map(JSON.parse);
  const waitEvent = async (name, predicate = () => true) => {
    for (;;) {
      const found = events().find((item) => item.event === name && predicate(item));
      if (found) return found;
      if (finished) throw new Error(`holder exited before ${name}: ${stdout} ${stderr}`);
      await new Promise((resolve) => waiting.push(resolve));
    }
  };
  const event = async (...args) => {
    let deadline;
    try {
      return await Promise.race([
        waitEvent(...args),
        new Promise((_, reject) => {
          deadline = setTimeout(
            () => reject(new Error(`waiting for ${args[0]}: ${stdout} ${stderr}`)),
            15000
          );
        }),
      ]);
    } finally {
      clearTimeout(deadline);
    }
  };
  const output = () => ({ stdout, stderr });
  return { child, ended, event, events, output };
}

export async function evalCommand(endpoint, args, cwd) {
  const holder = evalHolder(endpoint, args, cwd);
  holder.child.stdin.end();
  return holder.ended;
}
