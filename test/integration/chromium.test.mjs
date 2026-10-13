import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { frameId, launchBrowser, rawSocket } from './browser.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import { startServer } from './server.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const nativeDir = path.join(root, 'target/debug');
const wasmDir = path.join(root, 'target/wasm32-wasip1/debug');

test('chromium through the native binaries and the kernel', async (t) => {
  const site = await startServer();
  let browser;
  let kernel;
  try {
    browser = await launchBrowser([
      `--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1:${site.port}, MAP vault.sliccy.ai 127.0.0.1:${site.port}`,
      '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
    ]);
    const native = nativeRunner(
      path.join(nativeDir, process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'),
      path.join(nativeDir, process.platform === 'win32' ? 'curlwright.exe' : 'curlwright'),
      browser.wsUrl
    );
    kernel = await kernelRunner(
      path.join(wasmDir, 'playwright-cli.wasm'),
      path.join(wasmDir, 'curlwright.wasm'),
      browser.wsUrl
    );
    await t.test('native playwright-cli', () => playwrightFlow(native, site));
    await t.test('native curlwright', () => curlFlow(native, site, browser.wsUrl));
    await t.test('native route', () => routeFlow(native, site));
    await t.test('native protected tabs', () => protectedFlow(native, site, browser.wsUrl));
    await t.test('kernel playwright-cli', () => playwrightFlow(kernel, site));
    await t.test('kernel curlwright', () => curlFlow(kernel, site, browser.wsUrl));
    await t.test('kernel route', () => routeFlow(kernel, site));
    await t.test('kernel protected tabs', () => protectedFlow(kernel, site, browser.wsUrl));
  } finally {
    await settle(kernel?.close());
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

async function playwrightFlow(runner, site) {
  const bootDir = await runner.fresh();
  const booted = await runner.playwright(['open', `${site.origin}/boot`], bootDir);
  assert.equal(booted.code, 0, `${booted.stderr}\n${booted.stdout}`);
  const bootList = await runner.playwright(['requests'], bootDir);
  assert.equal(bootList.code, 0, `${bootList.stderr}\n${bootList.stdout}`);
  assert.match(bootList.stdout, /\/api\/boot/);
  const cwd = await runner.fresh();
  const page = `${site.origin}/page`;
  const opened = await runner.playwright(['open', page], cwd);
  assert.equal(opened.code, 0, opened.stderr);
  const openedId = targetOf(opened.stdout);
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
  assert.match(listed.stdout, new RegExp(escapeReg(`[${openedId}]`)));
  assert.match(listed.stdout, new RegExp(escapeReg(page)));
  const extra = await runner.playwright(['tab-new', `${site.origin}/other`], cwd);
  assert.equal(extra.code, 0, extra.stderr);
  const extraId = targetOf(extra.stdout);
  const againListed = await runner.playwright(['tab-list'], cwd);
  const pageLine = againListed.stdout.split('\n').find((line) => line.includes(`[${openedId}]`));
  const pageIndex = pageLine?.match(/^(\d+):/)?.[1];
  assert.ok(pageIndex, againListed.stdout);
  const selected = await runner.playwright(['tab-select', pageIndex], cwd);
  assert.equal(selected.code, 0, selected.stderr);
  assert.match(selected.stdout, new RegExp(escapeReg(openedId)));
  const moved = await runner.playwright(['goto', `${site.origin}/one`], cwd);
  assert.equal(moved.code, 0, moved.stderr);
  const logged = await runner.playwright(['eval', "console.error('slicc-console-marker')"], cwd);
  assert.equal(logged.code, 0, logged.stderr);
  const messages = await runner.playwright(['console'], cwd);
  assert.equal(messages.code, 0, `${messages.stderr}\n${messages.stdout}`);
  assert.equal(markerCount(messages.stdout), 1, messages.stdout);
  const reread = await runner.playwright(['snapshot'], cwd);
  assert.equal(reread.code, 0, `${reread.stderr}\n${reread.stdout}`);
  const replayed = await runner.playwright(['console'], cwd);
  assert.equal(replayed.code, 0, `${replayed.stderr}\n${replayed.stdout}`);
  assert.equal(markerCount(replayed.stdout), 1, replayed.stdout);
  const fetched = await runner.playwright(
    ['eval', `fetch('${site.origin}/api/me').then((response) => response.text())`],
    cwd
  );
  assert.equal(fetched.code, 0, `${fetched.stderr}\n${fetched.stdout}`);
  const requests = await runner.playwright(['requests'], cwd);
  assert.equal(requests.code, 0, `${requests.stderr}\n${requests.stdout}`);
  assert.match(requests.stdout, /\/api\/me/);
  const requestIndex = requests.stdout
    .split('\n')
    .find((line) => line.includes('/api/me'))
    ?.match(/^(\d+)\./)?.[1];
  assert.ok(requestIndex, requests.stdout);
  const one = await runner.playwright(['request', requestIndex], cwd);
  assert.equal(one.code, 0, `${one.stderr}\n${one.stdout}`);
  assert.match(one.stdout, /\/api\/me/);
  const missing = await runner.playwright(['request', '9999'], cwd);
  assert.equal(missing.code, 1);
  assert.match(missing.stderr, /not found/);
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
  const closedPage = await runner.playwright(['tab-close', '--tab', tab], cwd);
  assert.equal(closedPage.code, 0, closedPage.stderr);
  const closedOther = await runner.playwright(['tab-close', '--tab', other], cwd);
  assert.equal(closedOther.code, 0, closedOther.stderr);
}

async function routeFlow(runner, site) {
  const cwd = await runner.fresh();
  const ok = (result) => assert.equal(result.code, 0, `${result.stderr}\n${result.stdout}`);
  const empty = await runner.playwright(['route-list'], cwd);
  ok(empty);
  assert.equal(empty.stdout, 'No active routes\n');
  const refused = await runner.playwright(
    ['route', 'https://vault.sliccy.ai/**', '--status=200'],
    cwd
  );
  assert.equal(refused.code, 1);
  assert.equal(refused.stderr, "slicc: refused: target is on seven's origin\n");
  const mocked = await runner.playwright(
    ['route', '**/api/me', '--body=mocked', '--content-type=text/plain', '--header', 'X-Mock: yes'],
    cwd
  );
  ok(mocked);
  assert.equal(mocked.stdout, 'Route added for pattern: **/api/me\n');
  ok(
    await runner.playwright(
      ['route', '**/api/headers', '--header=X-Added: 1', '--remove-header=x-drop'],
      cwd
    )
  );
  await runner.write(cwd, 'mock.html', '<!doctype html><title>From file</title><p>file</p>');
  ok(await runner.playwright(['route', '**/two', '--fulfill-file', 'mock.html'], cwd));
  ok(await runner.playwright(['route', '*', '--status=418'], cwd));
  ok(await runner.playwright(['route', '**/*', '--status=418'], cwd));
  ok(await runner.playwright(['unroute', '**/*'], cwd));
  const listed = await runner.playwright(['route-list'], cwd);
  ok(listed);
  assert.equal(
    listed.stdout,
    '1. **/api/me (body=mocked, contentType=text/plain, addHeaders={"X-Mock":"yes"})\n2. **/api/headers (addHeaders={"X-Added":"1"}, removeHeaders=x-drop)\n3. **/two (file=mock.html)\n4. * (status=418)\n'
  );
  ok(await runner.playwright(['unroute', '*'], cwd));
  const opened = await runner.playwright(['open', `${site.origin}/page`], cwd);
  ok(opened);
  const tab = targetOf(opened.stdout);
  const me = await runner.playwright(
    [
      'eval',
      "fetch('/api/me').then(async (r) => (await r.text()) + ':' + r.headers.get('x-mock'))",
    ],
    cwd
  );
  ok(me);
  assert.equal(me.stdout, 'mocked:yes\n');
  const edited = await runner.playwright(
    ['eval', "fetch('/api/headers', { headers: { 'X-Drop': '1' } }).then((r) => r.text())"],
    cwd
  );
  ok(edited);
  assert.equal(edited.stdout, 'x-added=1 x-drop=none\n');
  ok(await runner.playwright(['route', '**', '--status=418'], cwd));
  const seven = await runner.playwright(
    ['eval', "fetch('http://seven.sliccy.ai/ping').then((r) => r.status + ':' + r.statusText)"],
    cwd
  );
  ok(seven);
  assert.equal(seven.stdout, '200:OK\n');
  ok(await runner.playwright(['unroute', '**'], cwd));
  ok(await runner.playwright(['goto', `${site.origin}/two`], cwd));
  const title = await runner.playwright(['eval', 'document.title'], cwd);
  ok(title);
  assert.equal(title.stdout, 'From file\n');
  const removed = await runner.playwright(['unroute', '**/api/me'], cwd);
  ok(removed);
  assert.equal(removed.stdout, 'Removed 1 route(s) for pattern: **/api/me\n');
  const real = await runner.playwright(['eval', "fetch('/api/me').then((r) => r.text())"], cwd);
  ok(real);
  assert.match(real.stdout, /session=from-tab/);
  const all = await runner.playwright(['unroute'], cwd);
  ok(all);
  assert.equal(all.stdout, 'Removed all 2 route(s)\n');
  ok(await runner.playwright(['tab-close', '--tab', tab], cwd));
}

async function protectedFlow(runner, site, wsUrl) {
  const raw = await rawSocket(wsUrl);
  const refusal = /^slicc: refused: /;
  const refused = async (args, cwd) => {
    const result = await runner.playwright(args, cwd);
    assert.equal(result.code, 1, `${args.join(' ')}\n${result.stdout}`);
    assert.match(result.stderr, refusal, args.join(' '));
    assert.doesNotMatch(result.stdout + result.stderr, /SECRET/);
  };
  try {
    const seven = await raw.open('http://seven.sliccy.ai/landing?k=SEVENSECRET');
    const vault = await raw.open('http://vault.sliccy.ai/landing?k=VAULTSECRET');
    const cwd = await runner.fresh();
    const listed = await runner.playwright(['tab-list'], cwd);
    assert.equal(listed.code, 0, listed.stderr);
    for (const hidden of [seven, vault, 'sliccy', 'SECRET']) {
      assert.doesNotMatch(listed.stdout, new RegExp(escapeReg(hidden)), listed.stdout);
    }
    for (const tab of [seven, vault]) {
      await refused(['eval', 'location.href', '--tab', tab], cwd);
      await refused(['snapshot', '--tab', tab], cwd);
      await refused(['screenshot', '--tab', tab, '--filename', 'x.png'], cwd);
      await refused(['press', 'a', '--tab', tab], cwd);
      await refused(['goto', `${site.origin}/one`, '--tab', tab], cwd);
      await refused(['tab-close', '--tab', tab], cwd);
      const curled = await runner.curl(['--tab', tab, `${site.origin}/api/me`], cwd);
      assert.notEqual(curled.code, 0, curled.stdout);
      assert.match(curled.stderr, /slicc: refused: /);
      assert.ok((await raw.targetUrl(tab)).includes('SECRET'), 'the protected tab moved');
    }
    await refused(['open', 'http://vault.sliccy.ai/'], cwd);
    await refused(['open', 'https://seven.sliccy.ai/os/'], cwd);
    const body =
      '<!doctype html><meta http-equiv="refresh" content="0;url=http://vault.sliccy.ai/landing?n=refresh"><title>refresh</title>';
    const routed = await runner.playwright(
      ['route', '**/refresh', `--body=${body}`, '--content-type=text/html'],
      cwd
    );
    assert.equal(routed.code, 0, routed.stderr);
    const opened = await runner.playwright(['open', `${site.origin}/page`], cwd);
    assert.equal(opened.code, 0, opened.stderr);
    const tab = targetOf(opened.stdout);
    await runner.playwright(['goto', `${site.origin}/refresh`], cwd);
    const deadline = Date.now() + 15000;
    for (;;) {
      const probe = await runner.playwright(['eval', 'location.href'], cwd);
      if (probe.code !== 0 && refusal.test(probe.stderr)) break;
      assert.ok(
        Date.now() < deadline,
        `the refreshed tab was never refused: ${probe.stdout}${probe.stderr}`
      );
      await new Promise((resolve) => setTimeout(resolve, 200));
    }
    assert.match(await raw.targetUrl(tab), /^http:\/\/vault\.sliccy\.ai\//);
    await refused(['eval', 'document.cookie', '--tab', tab], cwd);
    await refused(['snapshot', '--tab', tab], cwd);
    await refused(['screenshot', '--tab', tab, '--filename', 'y.png'], cwd);
    await refused(['press', 'a', '--tab', tab], cwd);
    await refused(['goto', `${site.origin}/one`, '--tab', tab], cwd);
    const afterList = await runner.playwright(['tab-list'], cwd);
    assert.doesNotMatch(afterList.stdout, new RegExp(escapeReg(tab)));
    const requests = await runner.playwright(['requests', '--tab', tab], cwd);
    assert.doesNotMatch(requests.stdout + requests.stderr, /vault|n=refresh/);
    await runner.playwright(['unroute'], cwd);
    for (const id of [seven, vault, tab]) await raw.send('Target.closeTarget', { targetId: id });
  } finally {
    raw.close();
  }
}

function markerCount(stdout) {
  return stdout.split('slicc-console-marker').length - 1;
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
