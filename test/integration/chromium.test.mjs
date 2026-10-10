import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { frameId, launchBrowser } from './browser.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import { startServer } from './server.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const nativeDir = path.join(root, 'target/debug');
const wasmDir = path.join(root, 'target/wasm32-wasip1/debug');

test('chromium through the native binaries and the kernel', async (t) => {
  const site = await startServer();
  const browser = await launchBrowser();
  const native = nativeRunner(
    path.join(nativeDir, process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'),
    path.join(nativeDir, process.platform === 'win32' ? 'curlwright.exe' : 'curlwright'),
    browser.wsUrl
  );
  const kernel = await kernelRunner(
    path.join(wasmDir, 'playwright-cli.wasm'),
    path.join(wasmDir, 'curlwright.wasm'),
    browser.wsUrl
  );
  try {
    await t.test('native playwright-cli', () => playwrightFlow(native, site));
    await t.test('native curlwright', () => curlFlow(native, site, browser.wsUrl));
    await t.test('kernel playwright-cli', () => playwrightFlow(kernel, site));
    await t.test('kernel curlwright', () => curlFlow(kernel, site, browser.wsUrl));
  } finally {
    await kernel.close();
    await browser.close();
    await site.close();
  }
});

async function playwrightFlow(runner, site) {
  const cwd = await runner.fresh();
  const page = `${site.origin}/page`;
  const opened = await runner.playwright(['open', page], cwd);
  assert.equal(opened.code, 0, opened.stderr);
  const shot = await runner.playwright(['snapshot'], cwd);
  assert.equal(shot.code, 0, `${shot.stderr}\n${shot.stdout}`);
  const button = refOf(shot.stdout, 'button', 'Go');
  const clicked = await runner.playwright(['click', button], cwd);
  assert.equal(clicked.code, 0, clicked.stderr);
  const afterClick = await runner.playwright(
    ['eval', "document.getElementById('out').textContent"],
    cwd
  );
  assert.equal(afterClick.code, 0, afterClick.stderr);
  assert.equal(afterClick.stdout, 'clicked\n');
  const again = await runner.playwright(['snapshot'], cwd);
  assert.equal(again.code, 0, again.stderr);
  const box = refOf(again.stdout, 'textbox', 'Name');
  const filled = await runner.playwright(['fill', box, 'ada'], cwd);
  assert.equal(filled.code, 0, filled.stderr);
  const typed = await runner.playwright(['type', 'x'], cwd);
  assert.equal(typed.code, 0, typed.stderr);
  const pressed = await runner.playwright(['press', 'Enter'], cwd);
  assert.equal(pressed.code, 0, pressed.stderr);
  const afterEnter = await runner.playwright(
    ['eval', "document.getElementById('out').textContent"],
    cwd
  );
  assert.equal(afterEnter.code, 0, `${afterEnter.stderr}\n${afterEnter.stdout}`);
  assert.equal(afterEnter.stdout, 'enter:adax\n');
  const image = await runner.playwright(['screenshot', '--filename', 'shot.png'], cwd);
  assert.equal(image.code, 0, image.stderr);
  const png = await runner.read(cwd, 'shot.png');
  assert.equal(png[0], 0x89);
  assert.equal(png[1], 0x50);
  const listed = await runner.playwright(['tab-list'], cwd);
  assert.equal(listed.code, 0, listed.stderr);
  assert.match(listed.stdout, new RegExp(escapeReg(page)));
  const extra = await runner.playwright(['tab-new', `${site.origin}/other`], cwd);
  assert.equal(extra.code, 0, extra.stderr);
  const extraId = targetOf(extra.stdout);
  const againListed = await runner.playwright(['tab-list'], cwd);
  const pageLine = againListed.stdout.split('\n').find((line) => line.includes(page));
  const pageIndex = pageLine?.match(/^(\d+):/)?.[1];
  assert.ok(pageIndex, againListed.stdout);
  const selected = await runner.playwright(['tab-select', pageIndex], cwd);
  assert.equal(selected.code, 0, selected.stderr);
  const moved = await runner.playwright(['goto', `${site.origin}/one`], cwd);
  assert.equal(moved.code, 0, moved.stderr);
  const closed = await runner.playwright(['tab-close', '--tab', extraId], cwd);
  assert.equal(closed.code, 0, closed.stderr);
  const finished = await runner.playwright(['close'], cwd);
  assert.equal(finished.code, 0, finished.stderr);
}

async function curlFlow(runner, site, wsUrl) {
  const cwd = await runner.fresh();
  const page = `${site.origin}/page`;
  const opened = await runner.playwright(['open', page], cwd);
  assert.equal(opened.code, 0, opened.stderr);
  const tab = targetOf(opened.stdout);
  const got = await runner.curl([`${site.origin}/api/me`], cwd);
  assert.equal(got.code, 0, got.stderr);
  assert.match(got.stdout, /session=from-tab/);
  const posted = await runner.curl(['--json', '{"a":1}', `${site.origin}/json-post`], cwd);
  assert.equal(posted.code, 0, `${posted.stderr}\n${posted.stdout}`);
  assert.equal(posted.stdout, '{"ok":true}');
  await runner.write(cwd, 'upload.bin', 'hello-bytes');
  const uploaded = await runner.curl(
    ['-F', 'up=@upload.bin;filename=up.bin', `${site.origin}/upload`],
    cwd
  );
  assert.equal(uploaded.code, 0, `${uploaded.stderr}\n${uploaded.stdout}`);
  assert.equal(uploaded.stdout, 'uploaded');
  const shown = await runner.curl(['-i', `${site.origin}/show`], cwd);
  assert.equal(shown.code, 0, shown.stderr);
  assert.match(shown.stdout, /^HTTP\/1\.1 200/);
  assert.match(shown.stdout, /x-test: yes/);
  assert.match(shown.stdout, /\r\n\r\nhi$/);
  const head = await runner.curl(['-I', `${site.origin}/only-head`], cwd);
  assert.equal(head.code, 0, head.stderr);
  assert.match(head.stdout, /HTTP\/1\.1 200/);
  assert.doesNotMatch(head.stdout, /SECRET/);
  const dumped = await runner.curl(['-D', 'headers.txt', `${site.origin}/dump`], cwd);
  assert.equal(dumped.code, 0, dumped.stderr);
  assert.equal(dumped.stdout, 'body-text');
  const headers = Buffer.from(await runner.read(cwd, 'headers.txt')).toString('utf8');
  assert.match(headers, /HTTP\/1\.1 200/);
  assert.match(headers, /x-dump: 1/);
  const code = await runner.curl(['-w', '%{http_code}', `${site.origin}/code`], cwd);
  assert.equal(code.code, 0, code.stderr);
  assert.equal(code.stdout, '201');
  const failed = await runner.curl(['-f', `${site.origin}/nope`], cwd);
  assert.equal(failed.code, 22, failed.stderr);
  assert.match(failed.stderr, /\(22\)/);
  const timed = await runner.curl(['-m', '1', '-w', '%{http_code}', `${site.origin}/slow`], cwd);
  assert.equal(timed.code, 28, `${timed.stderr}\n${timed.stdout}`);
  assert.match(timed.stderr, /\(28\)/);
  const second = await runner.playwright(['open', `${site.origin}/two`], cwd);
  assert.equal(second.code, 0, second.stderr);
  const other = targetOf(second.stdout);
  await runner.clearSession(cwd);
  const ambiguous = await runner.curl([`${site.origin}/api/me`], cwd);
  assert.equal(ambiguous.code, 2, ambiguous.stderr);
  assert.match(ambiguous.stderr, new RegExp(escapeReg(tab)));
  assert.match(ambiguous.stderr, new RegExp(escapeReg(other)));
  const picked = await runner.curl(['--tab', tab, `${site.origin}/api/me`], cwd);
  assert.equal(picked.code, 0, `${picked.stderr}\n${picked.stdout}`);
  assert.match(picked.stdout, /session=from-tab/);
  const frame = await frameId(wsUrl, tab);
  const framed = await runner.curl(['--tab', tab, '--frame', frame, `${site.origin}/api/me`], cwd);
  assert.equal(framed.code, 0, `${framed.stderr}\n${framed.stdout}`);
  const missing = await runner.curl(
    ['--tab', tab, '--frame', 'NOTAFRAME', `${site.origin}/api/me`],
    cwd
  );
  assert.notEqual(missing.code, 0, missing.stdout);
  assert.match(missing.stderr, /no frame/);
}

function targetOf(stdout) {
  const match = stdout.match(/\[targetId: ([^\]]+)\]/);
  if (!match) throw new Error(stdout);
  return match[1];
}

function refOf(stdout, role, name) {
  const match = stdout.match(new RegExp(`(?<![A-Za-z])${role} "${name}" \\[ref=(e\\d+)\\]`));
  if (!match) throw new Error(`missing ${role} ${name}\n${stdout}`);
  return match[1];
}

function escapeReg(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
