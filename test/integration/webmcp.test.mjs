import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { nativeRunner } from './runners.mjs';
import { startServer } from './server.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const nativeDir = path.join(root, 'target/debug');
const playwrightBin = path.join(
  nativeDir,
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);

test('webmcp-list and webmcp-call through the native binary', async (t) => {
  const site = await startServer();
  let browser;
  try {
    browser = await launchBrowser(['--enable-features=WebMCP']);
    const runner = nativeRunner(
      playwrightBin,
      path.join(nativeDir, process.platform === 'win32' ? 'curlwright.exe' : 'curlwright'),
      browser.wsUrl
    );
    await t.test('list tools on a WebMCP page', () => listToolsFlow(runner, site));
    await t.test('list human-readable without --json', () => listHumanFlow(runner, site));
    await t.test('call a tool on a WebMCP page', () => callToolFlow(runner, site));
    await t.test('call via stdin -', () => stdinFlow(runner, site, browser.wsUrl));
    await t.test('call with invalid JSON', () => invalidJsonFlow(runner, site));
    await t.test('call unknown tool', () => unknownToolFlow(runner, site));
    await t.test('call tool that throws', () => throwingToolFlow(runner, site));
    await t.test('consequential tool refused without flag (polyfill)', () =>
      consequentialRefusedFlow(runner, site)
    );
    await t.test('consequential tool allowed with flag (polyfill)', () =>
      consequentialAllowedFlow(runner, site)
    );
    await t.test('list on a page without WebMCP returns empty', () => emptyListFlow(runner, site));
    await t.test('call on a page without WebMCP exits 2', () => absentCallFlow(runner, site));
    await t.test('list declarative form tools', () => formToolsFlow(runner, site));
    await t.test('refuse protected tab', () => protectedRefusalFlow(runner, site));
    await t.test('canary: Chrome still rejects object executeTool input (#116)', () =>
      executeToolStringCanary(runner, site)
    );
  } finally {
    await settle(browser?.close());
    await settle(site.close());
  }
});

async function settle(value) {
  try {
    await value;
  } catch {
    return undefined;
  }
}

async function listToolsFlow(runner, site) {
  const cwd = await runner.fresh();
  const opened = await runner.playwright(['open', `${site.origin}/webmcp`], cwd);
  assert.equal(opened.code, 0, opened.stderr);
  const listed = await runner.playwright(['webmcp-list', '--json'], cwd);
  assert.equal(listed.code, 0, listed.stderr);
  const tools = JSON.parse(listed.stdout.trim());
  assert.ok(Array.isArray(tools));
  assert.ok(tools.length >= 2, `expected >= 2 tools, got ${tools.length}`);
  const greet = tools.find((t) => t.name === 'greet');
  assert.ok(greet, `greet not found in ${JSON.stringify(tools.map((t) => t.name))}`);
  assert.equal(greet.title, 'Greet');
  assert.equal(greet.description, 'Returns a greeting');
  assert.ok(greet.inputSchema);
  assert.equal(typeof greet.inputSchema, 'object');
  assert.ok(greet.origin);
}

async function listHumanFlow(runner, site) {
  const cwd = await runner.fresh();
  await runner.playwright(['open', `${site.origin}/webmcp`], cwd);
  const listed = await runner.playwright(['webmcp-list'], cwd);
  assert.equal(listed.code, 0, listed.stderr);
  assert.ok(listed.stdout.includes('greet'), listed.stdout);
  assert.ok(!listed.stdout.startsWith('['), 'should not be raw JSON');
}

async function callToolFlow(runner, site) {
  const cwd = await runner.fresh();
  await runner.playwright(['open', `${site.origin}/webmcp`], cwd);
  const called = await runner.playwright(['webmcp-call', 'greet', '{"name":"Alice"}'], cwd);
  assert.equal(called.code, 0, called.stderr);
  assert.equal(called.stdout.trim(), 'Hello, Alice!');
}

async function stdinFlow(runner, site, wsUrl) {
  const cwd = await runner.fresh();
  await runner.playwright(['open', `${site.origin}/webmcp`], cwd);
  const result = await runWithStdin(
    playwrightBin,
    ['--cdp', wsUrl, 'webmcp-call', 'greet', '-'],
    '{"name":"Stdin"}',
    cwd
  );
  assert.equal(result.code, 0, result.stderr);
  assert.equal(result.stdout.trim(), 'Hello, Stdin!');
}

async function invalidJsonFlow(runner, site) {
  const cwd = await runner.fresh();
  await runner.playwright(['open', `${site.origin}/webmcp`], cwd);
  const called = await runner.playwright(['webmcp-call', 'greet', 'not-json'], cwd);
  assert.equal(called.code, 2);
  assert.ok(called.stderr.includes('invalid JSON'));
}

async function unknownToolFlow(runner, site) {
  const cwd = await runner.fresh();
  await runner.playwright(['open', `${site.origin}/webmcp`], cwd);
  const called = await runner.playwright(['webmcp-call', 'nonexistent', '{}'], cwd);
  assert.equal(called.code, 2);
  assert.ok(called.stderr.includes('unknown tool'));
}

async function throwingToolFlow(runner, site) {
  const cwd = await runner.fresh();
  await runner.playwright(['open', `${site.origin}/webmcp`], cwd);
  const called = await runner.playwright(['webmcp-call', 'fail', '{}'], cwd);
  assert.equal(called.code, 1);
  assert.ok(called.stderr.includes('threw'), called.stderr);
}

async function consequentialRefusedFlow(runner, site) {
  const cwd = await runner.fresh();
  await runner.playwright(['open', `${site.origin}/webmcp-consequential`], cwd);
  const called = await runner.playwright(['webmcp-call', 'risky', '{}'], cwd);
  assert.equal(called.code, 2, `expected 2, got ${called.code}: ${called.stderr}`);
  assert.ok(called.stderr.includes('consequential'), called.stderr);
  assert.ok(called.stderr.includes('--allow-consequential'), called.stderr);
}

async function consequentialAllowedFlow(runner, site) {
  const cwd = await runner.fresh();
  await runner.playwright(['open', `${site.origin}/webmcp-consequential`], cwd);
  const called = await runner.playwright(
    ['webmcp-call', 'risky', '{}', '--allow-consequential'],
    cwd
  );
  assert.equal(called.code, 0, called.stderr);
  assert.equal(called.stdout.trim(), 'risky executed');
}

async function emptyListFlow(runner, site) {
  const cwd = await runner.fresh();
  await runner.playwright(['open', `${site.origin}/webmcp-empty`], cwd);
  const listed = await runner.playwright(['webmcp-list', '--json'], cwd);
  assert.equal(listed.code, 0, listed.stderr);
  const tools = JSON.parse(listed.stdout.trim());
  assert.ok(Array.isArray(tools));
  assert.equal(tools.length, 0);
}

async function absentCallFlow(runner, site) {
  const cwd = await runner.fresh();
  await runner.playwright(['open', `${site.origin}/webmcp-empty`], cwd);
  const called = await runner.playwright(['webmcp-call', 'anything', '{}'], cwd);
  assert.ok(called.code > 0);
  assert.ok(
    called.stderr.includes('absent') || called.stderr.includes('unknown tool'),
    called.stderr
  );
}

async function formToolsFlow(runner, site) {
  const cwd = await runner.fresh();
  await runner.playwright(['open', `${site.origin}/webmcp-form`], cwd);
  const listed = await runner.playwright(['webmcp-list', '--json'], cwd);
  assert.equal(listed.code, 0, listed.stderr);
  const tools = JSON.parse(listed.stdout.trim());
  const search = tools.find((t) => t.name === 'search');
  assert.ok(search, `search not found in ${JSON.stringify(tools.map((t) => t.name))}`);
  assert.ok(search.inputSchema);
  assert.ok(search.inputSchema.properties.query);
}

async function protectedRefusalFlow(runner, site) {
  const cwd = await runner.fresh();
  await runner.playwright(['open', `${site.origin}/webmcp`], cwd);
  const listed = await runner.playwright(
    ['webmcp-list', '--tab', 'nonexistent-protected-tab-id'],
    cwd
  );
  assert.equal(listed.code, 1);
  const called = await runner.playwright(
    ['webmcp-call', 'greet', '{}', '--tab', 'nonexistent-protected-tab-id'],
    cwd
  );
  assert.equal(called.code, 1);
}

function runWithStdin(bin, args, input, cwd) {
  return new Promise((resolve, reject) => {
    const env = { ...process.env };
    delete env.SLICC_CDP_URL;
    const child = spawn(bin, args, { cwd, env });
    const out = [];
    const err = [];
    child.stdout.on('data', (chunk) => out.push(chunk));
    child.stderr.on('data', (chunk) => err.push(chunk));
    child.stdin.write(input);
    child.stdin.end();
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      reject(new Error('stdin test timed out'));
    }, 30000);
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

async function executeToolStringCanary(runner, site) {
  const cwd = await runner.fresh();
  await runner.playwright(['open', `${site.origin}/webmcp`], cwd);
  const r = await runner.playwright(
    [
      'eval',
      `(async () => {
        const tools = await document.modelContext.getTools();
        const tool = tools.find(t => t.name === 'greet');
        if (!tool) return 'skip: no greet tool';
        try {
          await document.modelContext.executeTool(tool, {name: 'Canary'});
          return 'OBJECT_ACCEPTED';
        } catch (e) {
          return 'object_rejected: ' + e;
        }
      })()`,
    ],
    cwd
  );
  assert.equal(r.code, 0, r.stderr);
  assert.ok(
    !r.stdout.includes('OBJECT_ACCEPTED'),
    'Chrome now accepts object input to executeTool. ' +
      'Switch webmcp-call to pass an object instead of JSON.stringify. See #116.'
  );
}
