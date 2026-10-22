import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const nativeDir = path.join(root, 'target/debug');
const wasmDir = path.join(root, 'target/wasm32-wasip1/debug');
const exe = (name) => path.join(nativeDir, process.platform === 'win32' ? `${name}.exe` : name);

function site() {
  let port = 0;
  const pages = {
    '/fr.html': () => `<h1>Top</h1><iframe src="http://localhost:${port}/child.html"></iframe>`,
    '/child.html': () => '<button onclick="this.textContent=\'Pressed\'">Inner</button>',
    '/guarded.html': () =>
      '<h1>Top</h1><iframe src="http://seven.sliccy.ai/os/secret.html?k=OOPIF_QUERY_SECRET"></iframe>',
    '/os/secret.html': () => '<button>OOPIF_SEVEN_CONTENT</button>',
  };
  const server = createServer((req, res) => {
    const page = pages[new URL(req.url ?? '/', 'http://127.0.0.1').pathname];
    res.writeHead(page ? 200 : 404, { 'content-type': 'text/html' });
    res.end(page ? `<!doctype html>${page()}` : 'missing');
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      port = server.address().port;
      resolve({
        port,
        close: () =>
          new Promise((done) => {
            server.closeAllConnections();
            server.close(() => done());
          }),
      });
    });
  });
}

async function settle(value) {
  try {
    await value;
  } catch {
    return undefined;
  }
}

async function flow(runner, port) {
  const cwd = await runner.fresh();
  const run = async (args) => {
    const result = await runner.playwright(args, cwd);
    assert.equal(result.code, 0, `${args.join(' ')}: ${result.stderr}\n${result.stdout}`);
    return result;
  };
  await run(['open', `http://127.0.0.1:${port}/fr.html`]);
  let shot = '';
  for (let attempt = 0; attempt < 20 && !/button "Inner"/.test(shot); attempt += 1) {
    if (attempt) await new Promise((resolve) => setTimeout(resolve, 250));
    shot = (await run(['snapshot'])).stdout;
  }
  assert.match(shot, /- heading "Top" \[level=1\] \[ref=e2\]\n {2}- iframe \[ref=e3\]:\n/);
  const ref = shot.match(/button "Inner" \[ref=(f1e\d+)\]/)?.[1];
  assert.ok(ref, shot);
  const frames = await run(['frames']);
  assert.match(frames.stdout, /\[main\] frameId=\S+ - http:\/\/127\.0\.0\.1:\d+\/fr\.html\n/);
  assert.match(
    frames.stdout,
    /\[child\] frameId=\S+ parentFrameId=\S+ - http:\/\/localhost:\d+\/child\.html\n/
  );
  assert.equal((await run(['click', ref])).stdout, `Clicked ${ref}\n`);
  assert.match((await run(['snapshot'])).stdout, /button "Pressed" \[ref=f1e\d+\]/);
  await run(['goto', `http://127.0.0.1:${port}/guarded.html`]);
  await new Promise((resolve) => setTimeout(resolve, 1000));
  const guarded = await run(['snapshot']);
  assert.match(guarded.stdout, /- heading "Top" \[level=1\] \[ref=e2\]\n {2}- iframe \[ref=e3\]/);
  const listed = await run(['frames']);
  for (const output of [guarded, listed]) {
    assert.doesNotMatch(
      output.stdout + output.stderr,
      /OOPIF_SEVEN_CONTENT|OOPIF_QUERY_SECRET|secret\.html|\/os\//
    );
  }
}

test('a cross-origin iframe is in snapshot and frames, and its ref clicks, under site isolation', async (t) => {
  const server = await site();
  let browser;
  let kernel;
  try {
    browser = await launchBrowser([
      '--site-per-process',
      `--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1:${server.port}`,
      '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
    ]);
    const native = nativeRunner(exe('playwright-cli'), exe('curlwright'), browser.wsUrl);
    await t.test('native', () => flow(native, server.port));
    kernel = await kernelRunner(
      path.join(wasmDir, 'playwright-cli.wasm'),
      path.join(wasmDir, 'curlwright.wasm'),
      browser.wsUrl
    );
    await t.test('kernel', () => flow(kernel, server.port));
  } finally {
    await settle(kernel?.close());
    await settle(browser?.close());
    await settle(server.close());
  }
});
