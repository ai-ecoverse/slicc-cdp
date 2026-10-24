import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { createServer } from 'node:https';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { launchBrowser, rawSocket } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import { startServer } from './server.mjs';
import './watchdog.mjs';

const exe = (name) =>
  path.resolve('target/debug', process.platform === 'win32' ? `${name}.exe` : name);
const wasm = (name) => path.resolve('target/wasm32-wasip1/debug', `${name}.wasm`);

function sevenServer() {
  const scratch = mkdtempSync(path.join(tmpdir(), 'slicc-headed-'));
  const key = path.join(scratch, 'key.pem');
  const cert = path.join(scratch, 'cert.pem');
  execFileSync(
    'openssl',
    ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', key, '-out', cert].concat([
      '-days',
      '1',
      '-subj',
      '/CN=seven.sliccy.ai',
    ]),
    { stdio: 'ignore' }
  );
  const server = createServer({ key: readFileSync(key), cert: readFileSync(cert) }, (_, res) => {
    res.setHeader('content-type', 'text/html');
    res.end('<!doctype html><title>Seven</title><p>seven</p>');
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () =>
      resolve({
        port: server.address().port,
        close: async () => {
          server.closeAllConnections();
          await new Promise((done) => server.close(done));
          rmSync(scratch, { recursive: true, force: true });
        },
      })
    );
  });
}

function refOf(stdout, role, name) {
  const ref = stdout.match(new RegExp(`${role} "${name}"(?: \\[[^\\]]+\\])*? \\[ref=(e\\d+)\\]`));
  assert.ok(ref, `${role} ${name}\n${stdout}`);
  return ref[1];
}

async function inTab(raw, targetId, expression) {
  const { sessionId } = await raw.send('Target.attachToTarget', { targetId, flatten: true });
  try {
    const { result } = await raw.send(
      'Runtime.evaluate',
      { expression, returnByValue: true },
      sessionId
    );
    return result.value;
  } finally {
    await raw.send('Target.detachFromTarget', { sessionId }).catch(() => {});
  }
}

async function until(read, expected, label) {
  const deadline = Date.now() + 10000;
  let last;
  while (Date.now() < deadline) {
    last = await read();
    if (typeof expected === 'function' ? expected(last) : last === expected) return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  assert.fail(`${label}: ${JSON.stringify(last)}`);
}

async function flow(runner, raw, site, seven) {
  const cwd = await runner.fresh();
  const pw = async (args) => {
    const result = await runner.playwright(args, cwd);
    assert.equal(result.code, 0, `${args.join(' ')}: ${result.stderr}\n${result.stdout}`);
    return result.stdout;
  };
  const out = async () => (await pw(['eval', "document.getElementById('out').textContent"])).trim();
  const tab = (await pw(['open', `${site.origin}/page`])).match(/\[targetId: ([^\]]+)\]/)?.[1];
  assert.ok(tab);
  try {
    await steps(pw, out, raw, tab, seven);
  } finally {
    const attached = await raw
      .send('Target.attachToTarget', { targetId: tab, flatten: true })
      .catch(() => ({}));
    await raw
      .send('Page.handleJavaScriptDialog', { accept: false }, attached.sessionId)
      .catch(() => {});
    await raw.send('Target.closeTarget', { targetId: tab }).catch(() => {});
  }
}

async function steps(pw, out, raw, tab, seven) {
  const visibility = (target) => inTab(raw, target, 'document.visibilityState');
  assert.equal(await visibility(tab), 'hidden');
  assert.equal(await visibility(seven), 'visible');

  await pw(['click', refOf(await pw(['snapshot']), 'button', 'Go')]);
  await until(out, 'clicked', 'click');
  await pw(['hover', refOf(await pw(['snapshot']), 'button', 'Go')]);
  await until(out, 'hovered', 'hover');
  await pw(['dblclick', refOf(await pw(['snapshot']), 'button', 'Go')]);
  await until(out, 'dblclicked', 'dblclick');
  const agree = () => inTab(raw, tab, "document.getElementById('agree').checked");
  await pw(['check', refOf(await pw(['snapshot']), 'checkbox', 'Agree')]);
  await until(agree, true, 'check');
  await pw(['uncheck', refOf(await pw(['snapshot']), 'checkbox', 'Agree')]);
  await until(agree, false, 'uncheck');
  await pw(['fill', refOf(await pw(['snapshot']), 'textbox', 'Name'), 'ada']);
  await pw(['type', 'x']);
  await pw(['press', 'Enter']);
  await until(out, 'enter:adax', 'type and press');
  assert.equal(await visibility(seven), 'visible');

  await pw(['press', 'Tab']);
  await until(() => inTab(raw, tab, 'document.activeElement.id'), 'color', 'Tab moved focus');
  await until(() => visibility(seven), 'visible', 'seven is back in front');
  assert.equal(await visibility(tab), 'hidden');

  await inTab(
    raw,
    tab,
    'document.body.insertAdjacentHTML(\'beforeend\', \'<form onsubmit="window.submitted = true; return false"><textarea aria-label="Message"></textarea><button>Send</button></form>\')'
  );
  await pw(['fill', refOf(await pw(['snapshot']), 'textbox', 'Message'), 'hi']);
  await pw(['press', 'Enter']);
  await until(
    () => inTab(raw, tab, "document.querySelector('textarea').value"),
    'hi\n',
    'Enter in a textarea'
  );
  assert.equal(await inTab(raw, tab, 'window.submitted'), undefined);

  await inTab(
    raw,
    tab,
    "document.getElementById('go').onclick = () => { window.answer = confirm('sure?'); }"
  );
  const { sessionId } = await raw.send('Target.attachToTarget', { targetId: tab, flatten: true });
  try {
    await raw.send('Page.enable', {}, sessionId);
    const opening = raw.event('Page.javascriptDialogOpening', sessionId);
    const button = refOf(await pw(['snapshot']), 'button', 'Go');
    const clicked = pw(['click', button]);
    const dialog = await Promise.race([
      opening.seen,
      clicked.then(() => new Promise((resolve) => setTimeout(() => resolve(null), 10000))),
    ]);
    opening.cancel();
    assert.equal(dialog?.message, 'sure?');
    await raw.send('Page.handleJavaScriptDialog', { accept: true }, sessionId);
    await clicked;
  } finally {
    await raw.send('Target.detachFromTarget', { sessionId }).catch(() => {});
  }
  await until(() => inTab(raw, tab, 'window.answer'), true, 'the dialog answered the click');
  assert.equal(await visibility(seven), 'visible');
  await rawInput(pw, raw, tab, seven);
}

const PAD = `<div id="pad" style="position:fixed;left:0;top:0;width:600px;height:200px;background:#fff;z-index:9"><div id="src" role="button" aria-label="Source" style="position:absolute;left:20px;top:20px;width:100px;height:60px"></div><div id="dst" role="button" aria-label="Target" style="position:absolute;left:300px;top:20px;width:100px;height:60px"></div><input id="field" aria-label="Field" style="position:absolute;left:20px;top:120px"><textarea id="area" style="position:absolute;left:300px;top:120px"></textarea><form id="form" style="position:absolute;left:20px;top:160px"><input id="inform"></form></div>`;
const LOG = `window.log = []; for (const type of ['mousedown', 'mouseup', 'click', 'keydown', 'keyup']) document.getElementById('pad').addEventListener(type, (event) => log.push(type + ':' + event.target.id + (event.shiftKey ? ':shift' : '')), true); document.getElementById('pad').addEventListener('mousemove', (event) => { window.moved = event.target.id + ':' + event.buttons; }, true);`;

async function rawInput(pw, raw, tab, seven) {
  const visibility = (target) => inTab(raw, target, 'document.visibilityState');
  const log = () => inTab(raw, tab, "window.log.join(' ')");
  const clear = () => inTab(raw, tab, "window.log.length = 0; window.moved = ''");
  await inTab(
    raw,
    tab,
    `document.body.insertAdjacentHTML('beforeend', ${JSON.stringify(PAD)}); ${LOG}`
  );
  const shot = await pw(['snapshot']);
  await pw(['drag', refOf(shot, 'button', 'Source'), refOf(shot, 'button', 'Target')]);
  await until(log, 'mousedown:src mouseup:dst click:pad', 'drag');
  assert.equal(await inTab(raw, tab, 'window.moved'), 'dst:1');

  await clear();
  await pw(['mousemove', '70', '50']);
  await until(() => inTab(raw, tab, 'window.moved'), 'src:0', 'mousemove');
  await pw(['mousedown']);
  await pw(['mousemove', '350', '50']);
  await until(() => inTab(raw, tab, 'window.moved'), 'dst:1', 'mousemove with the button held');
  await pw(['mouseup']);
  await until(log, 'mousedown:src mouseup:dst click:pad', 'mousedown and mouseup');

  await clear();
  await inTab(raw, tab, "document.getElementById('field').focus()");
  for (const [command, key] of [
    ['keydown', 'Shift'],
    ['keydown', 'KeyA'],
    ['keyup', 'KeyA'],
    ['keyup', 'Shift'],
  ])
    await pw([command, key]);
  await until(
    log,
    'keydown:field:shift keydown:field:shift keyup:field:shift keyup:field',
    'keydown and keyup'
  );
  assert.equal(await inTab(raw, tab, "document.getElementById('field').value"), 'A');
  await inTab(
    raw,
    tab,
    "document.getElementById('form').onsubmit = () => { window.formSent = true; return false; }"
  );
  for (const target of ['area', 'inform']) {
    await inTab(raw, tab, `document.getElementById('${target}').focus()`);
    await pw(['keydown', 'Shift']);
    await pw(['keyup', 'Shift']);
    await pw(['press', 'Shift']);
  }
  assert.equal(await inTab(raw, tab, "document.getElementById('area').value"), '');
  assert.equal(await inTab(raw, tab, 'window.formSent'), undefined);
  assert.equal(await visibility(seven), 'visible');
  assert.equal(await visibility(tab), 'hidden');

  await pw(['keydown', 'Tab']);
  await until(
    () => inTab(raw, tab, 'document.activeElement.id'),
    (id) => id !== 'field',
    'Tab moved focus'
  );
  await pw(['keyup', 'Tab']);
  await until(() => visibility(seven), 'visible', 'seven is back in front after the raw Tab');
  assert.equal(await visibility(tab), 'hidden');
}

test('headed Chrome takes playwright-cli input on a background tab and keeps seven in front', {
  timeout: 300000,
}, async (t) => {
  const site = await startServer();
  const tls = await sevenServer();
  let browser;
  let raw;
  let kernel;
  try {
    browser = await launchBrowser(
      [
        `--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1:${tls.port}`,
        '--ignore-certificate-errors',
        '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
      ],
      { headless: false }
    );
    assert.equal(browser.headless, false);
    raw = await rawSocket(browser.wsUrl);
    const { targetId: seven } = await raw.send('Target.createTarget', {
      url: 'https://seven.sliccy.ai/',
      background: false,
    });
    await until(() => inTab(raw, seven, 'document.title'), 'Seven', 'seven loaded');
    await until(() => inTab(raw, seven, 'document.visibilityState'), 'visible', 'seven in front');
    const native = nativeRunner(exe('playwright-cli'), exe('curlwright'), browser.wsUrl);
    await t.test('native', () => flow(native, raw, site, seven));
    kernel = await kernelRunner(wasm('playwright-cli'), wasm('curlwright'), browser.wsUrl);
    await t.test('kernel', () => flow(kernel, raw, site, seven));
  } finally {
    kernel?.close();
    raw?.close();
    await browser?.close();
    await tls.close();
    await site.close();
  }
});
