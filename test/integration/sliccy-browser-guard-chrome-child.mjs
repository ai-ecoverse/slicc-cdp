import assert from 'node:assert/strict';
import { appendFile, mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import browser, { REFUSAL } from '../../packages/sliccy-browser/src/browser.js';

const site = process.env.GUARD_SITE;
const seven = process.env.GUARD_SEVEN;
const slack = process.env.GUARD_SLACK;
const edge = process.env.GUARD_EDGE;
const protectedWs = process.env.GUARD_PROTECTED_WS;
const allowedWs = process.env.GUARD_ALLOWED_WS;
const secretFile = process.env.GUARD_SECRET_FILE;
const earlyTargetId = process.env.GUARD_EARLY_TARGET;
const sevenTargetId = process.env.GUARD_SEVEN_TARGET;
const logPath = process.env.GUARD_LOG;

async function log(line) {
  await appendFile(logPath, `${line}\n`);
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitFor(predicate, label, ms = 8000) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await delay(50);
  }
  throw new Error(label);
}

async function expectRefuse(label, work) {
  await assert.rejects(work, { message: REFUSAL }, label);
  await log(`refuse:${label}`);
}

async function main() {
  await mkdir(path.dirname(logPath), { recursive: true });
  await writeFile(logPath, '');

  await expectRefuse('first-eval-seven', () => browser.eval(sevenTargetId, '1+1'));
  await expectRefuse('first-cookie-seven', () => browser.cookie(sevenTargetId, 'seven_session'));
  await expectRefuse('first-localStorage-seven', () =>
    browser.localStorage(sevenTargetId, 'canary')
  );
  await expectRefuse('first-fetch-seven', () => browser.fetch(sevenTargetId, `${seven}/ping`));

  await expectRefuse('file-findTab', () => browser.findTab({ urlMatch: '^file:' }));
  await expectRefuse('chrome-settings-findTab', () =>
    browser.findTab({ urlMatch: '^chrome://settings' })
  );
  await expectRefuse('file-ensureTab', () => browser.ensureTab(pathToFileURL(secretFile).href));
  await expectRefuse('chrome-settings-ensureTab', () => browser.ensureTab('chrome://settings/'));

  const framed = await browser.ensureTab(`${site}/framed-seven`, {
    matchUrl: '/framed-seven$',
  });
  assert.equal(await browser.cookie(framed, 'seven_session'), null);
  await browser.eval(
    framed,
    `document.body.innerHTML=${JSON.stringify(`<iframe src="${seven}/landing"></iframe>`)}`
  );
  await delay(500);
  assert.equal(await browser.cookie(framed, 'seven_session'), null);
  await log('cookie:framed-empty');

  const probeHitsBefore = Number(
    await (await fetch(`${site}/hits?path=${encodeURIComponent('/probe-canary')}`)).text()
  );
  const based = await browser.ensureTab(`${site}/base-seven`, { matchUrl: '/base-seven$' });
  assert.equal(await browser.eval(based, 'document.baseURI'), 'https://seven.sliccy.ai/');
  await expectRefuse('fetch-base-href', () => browser.fetch(based, '/probe-canary'));
  const probeHitsAfter = Number(
    await (await fetch(`${site}/hits?path=${encodeURIComponent('/probe-canary')}`)).text()
  );
  assert.equal(probeHitsAfter, probeHitsBefore);
  await log('fetch:base-no-request');

  await expectRefuse('ensureTab-seven', () => browser.ensureTab(`${seven}/landing`));
  await expectRefuse('ensureTab-redirect-seven', () =>
    browser.ensureTab(`${site}/redirect-seven`, { matchUrl: '/redirect-seven$' })
  );

  const earlyLogged = [];
  const write = console.log;
  console.log = (...args) => {
    earlyLogged.push(args.map(String).join(' '));
  };
  let bare;
  let matched;
  try {
    bare = await browser.websocket
      .on(earlyTargetId)
      .filter({ parseAs: 'json' })
      .forward({ sink: 'log' });
    matched = await browser.websocket
      .on(earlyTargetId, { urlMatch: /sliccy\.ai/u })
      .filter({ parseAs: 'json' })
      .forward({ sink: 'log' });
    await browser.eval(earlyTargetId, 'window.__guardEarly.send("ping")');
    await delay(800);
    assert.equal(
      earlyLogged.some((line) => line.includes('canary') || line.includes('"n":42')),
      false
    );
    await log('websocket:early-dropped');
  } finally {
    console.log = write;
    await bare?.close();
    await matched?.close();
  }

  const plain = await browser.ensureTab(`${site}/plain`, { matchUrl: '/plain$' });
  await browser.eval(plain, `location.href=${JSON.stringify(`${seven}/landing`)}`);
  await delay(400);
  await expectRefuse('nav-to-seven-eval', () => browser.eval(plain, '1+1'));
  await expectRefuse('nav-to-seven-cookie', () => browser.cookie(plain, 'seven_session'));

  const slackTab = await browser.findTab({
    domain: 'app.slack.com',
    urlMatch: /\/client\/[A-Z0-9]+/u,
  });
  assert.ok(slackTab?.targetId);
  const relative = await browser.fetch(slackTab, '/api/auth.test', { method: 'POST' });
  assert.equal(relative?.status, 200);
  assert.equal(relative?.body?.ok ?? relative?.body, true);
  const absolute = await browser.fetch(slackTab, edge);
  assert.equal(absolute?.status, 200);
  assert.equal(await browser.cookie(slackTab, 'd'), 'slack-d');
  assert.equal(await browser.localStorage(slackTab, 'localConfig_v2'), '{"ok":true}');
  await log('slack:positives');

  const allowedLogged = [];
  console.log = (...args) => {
    allowedLogged.push(args.map(String).join(' '));
  };
  let allowedHandle;
  try {
    const allowedTab = await browser.ensureTab(`${site}/plain2`, { matchUrl: '/plain2$' });
    allowedHandle = await browser.websocket
      .on(allowedTab, { urlMatch: new RegExp(allowedWs.replace(/[.*+?^${}()|[\]\\]/gu, '\\$&')) })
      .filter({ parseAs: 'json', where: { kind: 'tick' } })
      .forward({ sink: 'log' });
    await browser.evalAsync(
      allowedTab,
      `new Promise((resolve, reject) => {
        const ws = new WebSocket(${JSON.stringify(allowedWs)});
        const timer = setTimeout(() => reject(new Error('ws timeout')), 8000);
        ws.onerror = () => { clearTimeout(timer); reject(new Error('ws error')); };
        ws.onopen = () => { ws.send('ping'); clearTimeout(timer); resolve('opened'); };
      })`
    );
    await waitFor(
      () => allowedLogged.some((line) => line.includes('"n":1') || line.includes('"n": 1')),
      `allowed ws missed ${JSON.stringify(allowedLogged)}`
    );
    await log('websocket:allowed');
  } finally {
    console.log = write;
    await allowedHandle?.close();
  }

  assert.ok(protectedWs.includes('sliccy.ai'));
  assert.ok(slack.includes('app.slack.com'));
  await log('done');
  console.log('OK');
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
