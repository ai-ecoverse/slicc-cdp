import { spawn } from 'node:child_process';
import { realpathSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

export function verdictOf(name, stdout, accept = ['PASS']) {
  const prefix = `${name}: `;
  for (const line of stdout.split(/\r?\n/)) {
    if (!line.startsWith(prefix)) continue;
    const word = line.slice(prefix.length).split(' ')[0];
    if (accept.includes(word)) return line;
  }
  return null;
}

export function tapVerdict(name, stdout) {
  const fail = stdout.match(/^# fail (\d+)$/m);
  if (!fail) return { line: null, reason: 'no TAP summary' };
  if (Number(fail[1]) !== 0) return { line: null, reason: `${fail[1]} failed` };
  const ran = stdout
    .split(/\r?\n/)
    .map((line) => line.match(/^\s*ok \d+ - (.*)$/)?.[1])
    .filter((title) => title !== undefined)
    .filter((title) => !/ # (SKIP|TODO)\b/i.test(title))
    .filter((title) => !/^(\/|[A-Za-z]:\\).*\.[cm]?js$/.test(title)).length;
  if (ran === 0) return { line: null, reason: 'no test ran' };
  return { line: `${name}: PASS ${ran} tests`, reason: null };
}

export function parse(argv) {
  const split = argv.indexOf('--');
  if (split < 1 || split === argv.length - 1)
    throw new Error('usage: gate.mjs <name> [--tap] [--accept A,B] -- <command> [args...]');
  const [name, ...flags] = argv.slice(0, split);
  const command = argv.slice(split + 1);
  let tap = false;
  let accept = ['PASS'];
  for (let i = 0; i < flags.length; i++) {
    if (flags[i] === '--tap') tap = true;
    else if (flags[i] === '--accept') accept = flags[++i].split(',');
    else throw new Error(`gate.mjs: unknown flag ${flags[i]}`);
  }
  return { name, tap, accept, command };
}

export function judge({ name, tap, accept }, code, stdout) {
  if (code !== 0) return { ok: false, line: `${name}: FAIL exit ${code}` };
  if (tap) {
    const { line, reason } = tapVerdict(name, stdout);
    return line ? { ok: true, line } : { ok: false, line: `${name}: FAIL ${reason}` };
  }
  const line = verdictOf(name, stdout, accept);
  return line
    ? { ok: true, line: null }
    : { ok: false, line: `${name}: FAIL no "${name}: ${accept.join('|')}" verdict line` };
}

async function main() {
  const options = parse(process.argv.slice(2));
  const [file, ...args] = options.command;
  const child = spawn(file, args, {
    stdio: ['inherit', 'pipe', 'inherit'],
    shell: process.platform === 'win32',
  });
  const chunks = [];
  child.stdout.on('data', (chunk) => {
    chunks.push(chunk);
    process.stdout.write(chunk);
  });
  const code = await new Promise((resolve) => {
    child.on('error', () => resolve(127));
    child.on('close', (status, signal) => resolve(signal ? 128 : (status ?? 1)));
  });
  const verdict = judge(options, code, Buffer.concat(chunks).toString('utf8'));
  if (verdict.line) (verdict.ok ? process.stdout : process.stderr).write(`${verdict.line}\n`);
  process.exitCode = verdict.ok ? 0 : 1;
}

function invoked() {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

if (invoked()) await main();
