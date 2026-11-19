import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const exe = (name) =>
  path.join(root, 'target/debug', process.platform === 'win32' ? `${name}.exe` : name);

function stallingBrowser() {
  const sockets = [];
  const server = createServer((req, res) => {
    const { port } = server.address();
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(
      JSON.stringify({ webSocketDebuggerUrl: `ws://127.0.0.1:${port}/devtools/browser/stall` })
    );
  });
  server.on('upgrade', (_req, socket) => sockets.push(socket));
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () =>
      resolve({
        url: `http://127.0.0.1:${server.address().port}`,
        close: () =>
          new Promise((done) => {
            for (const socket of sockets) socket.destroy();
            server.closeAllConnections();
            server.close(() => done());
          }),
      })
    );
  });
}

function firstStderrLine(file, args) {
  return new Promise((resolve, reject) => {
    const child = spawn(file, args, { stdio: ['ignore', 'ignore', 'pipe'] });
    let text = '';
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      reject(
        new Error(`no stderr line before the stalled handshake ended: ${JSON.stringify(text)}`)
      );
    }, 5000);
    child.stderr.on('data', (chunk) => {
      text += chunk;
      if (text.includes('\n')) {
        clearTimeout(timer);
        const running = child.exitCode === null && child.signalCode === null;
        child.kill('SIGKILL');
        resolve({ line: text.split('\n')[0], running });
      }
    });
    child.on('error', reject);
  });
}

for (const [name, args] of [
  ['curlwright', (url) => ['-v', '--cdp', url, 'https://app.example/x']],
  ['playwright-cli', (url) => ['--verbose', '--cdp', url, 'tab-list']],
]) {
  test(`${name} names the chosen endpoint before a stalled handshake finishes`, async () => {
    const browser = await stallingBrowser();
    try {
      const { line, running } = await firstStderrLine(exe(name), args(browser.url));
      assert.equal(line, `* CDP ${browser.url}/json/version (via --cdp)`);
      assert.ok(running, 'the line came only after the process ended');
    } finally {
      await browser.close();
    }
  });
}
