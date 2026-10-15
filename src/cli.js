#!/usr/bin/env node
import { readFileSync, realpathSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { browsersRoot, findBrowser, installBrowser } from './browser.js';

export const USAGE = `usage: slicc-cdp <command> [options]

commands:
  install-browser   download Chrome for Testing, verify it, print its path
      --version <x.y.z.w|milestone>   pin a full version (recommended) or a milestone
      --channel <stable|beta|dev|canary>   without --version (default stable)
      --path <dir>                    install root (default $SLICC_BROWSERS_PATH or the user cache)
      --with-deps                     on Linux, apt-get Chrome's packages (needs root or sudo -n)
      --force                         download again even when installed
  find-browser      print the browser launchBrowser() uses: $CHROME_PATH, else the
                    newest installed Chrome for Testing, else a system Chrome
      --version <x.y.z.w|milestone>   only that installed Chrome for Testing ($CHROME_PATH is not used)
  --version, -V     print the slicc-cdp version
`;

export const VERSION = JSON.parse(
  readFileSync(new URL('../package.json', import.meta.url), 'utf8')
).version;

export async function main(
  argv,
  io = { out: process.stdout, err: process.stderr, env: process.env }
) {
  const [command, ...rest] = argv;
  if (!command || command === '--help' || command === '-h' || command === 'help') {
    io.out.write(USAGE);
    return command ? 0 : 2;
  }
  if (command === '--version' || command === '-V') {
    io.out.write(`slicc-cdp ${VERSION}\n`);
    return 0;
  }
  if (command === 'install-browser') {
    const { values } = parseArgs({
      args: rest,
      options: {
        channel: { type: 'string', default: 'stable' },
        version: { type: 'string' },
        path: { type: 'string' },
        'with-deps': { type: 'boolean', default: false },
        force: { type: 'boolean', default: false },
      },
    });
    const result = await installBrowser({
      channel: values.channel,
      version: values.version,
      root: values.path ?? browsersRoot(io.env),
      withDeps: values['with-deps'],
      force: values.force,
      log: (line) => io.err.write(`${line}\n`),
    });
    if (!result.installed) {
      io.err.write(`Chrome for Testing ${result.version} is already installed\n`);
    }
    io.out.write(`${result.executable}\n`);
    return 0;
  }
  if (command === 'find-browser') {
    const { values } = parseArgs({ args: rest, options: { version: { type: 'string' } } });
    const found = await findBrowser({ env: io.env, version: values.version });
    if (!found) {
      io.err.write(
        'no Chrome or Chromium found: set CHROME_PATH or run `slicc-cdp install-browser`\n'
      );
      return 1;
    }
    io.out.write(`${found.executable}\n`);
    return 0;
  }
  io.err.write(`slicc-cdp: unknown command "${command}"\n${USAGE}`);
  return 2;
}

const invoked = process.argv[1] && realpathSync(process.argv[1]) === fileURLToPath(import.meta.url);
if (invoked) {
  main(process.argv.slice(2)).then(
    (code) => {
      process.exitCode = code;
    },
    (error) => {
      process.stderr.write(`slicc-cdp: ${error.message}\n`);
      process.exitCode = 1;
    }
  );
}
