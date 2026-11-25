import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { createServer } from 'node:https';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchBrowser, rawSocket } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { harCommand, harHolder } from './har-support.mjs';
import './watchdog.mjs';

const directory = fileURLToPath(new URL('.', import.meta.url));
const tls = {
  cert: await readFile(path.join(directory, 'fixtures/tls/cert.pem')),
  key: await readFile(path.join(directory, 'fixtures/tls/key.pem')),
};
const FETCHES = 6;

test('HAR records Chrome fetches that each open a new TLS connection', {
  timeout: 180000,
}, async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-har-tls-chrome-'));
  const server = createServer(tls, (req, res) => {
    const html = req.url === '/page';
    res.writeHead(200, {
      'content-type': html ? 'text/html' : 'application/json',
      'access-control-allow-origin': '*',
      connection: 'close',
    });
    res.end(html ? '<!doctype html><title>TLS fixture</title>' : '{"ok":true}');
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const port = server.address().port;
  const origin = `https://ordinary.tls.test:${port}`;
  let browser;
  let raw;
  let held;
  const seen = [];
  try {
    browser = await launchBrowser([
      '--ignore-certificate-errors',
      '--no-proxy-server',
      `--host-resolver-rules=MAP ordinary.tls.test 127.0.0.1:${port}`,
      '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
    ]);
    raw = await rawSocket(browser.wsUrl);
    const opened = await harCommand(browser.wsUrl, ['tab-new', 'about:blank'], home);
    assert.equal(opened.code, 0, opened.stderr);
    const target = opened.stdout.match(/targetId: ([^\]]+)\]/)?.[1];
    assert.ok(target, opened.stdout);
    const { sessionId } = await raw.send('Target.attachToTarget', {
      targetId: target,
      flatten: true,
    });
    await raw.send('Page.enable', {}, sessionId);
    await raw.send('Network.enable', {}, sessionId);
    const file = path.join(home, 'traffic.har');
    held = harHolder(browser.wsUrl, ['har-start', '--tab', target, file], home);
    await held.event('ready');
    const load = raw.event('Page.loadEventFired', sessionId);
    await raw.send('Page.navigate', { url: `${origin}/page` }, sessionId);
    await load.seen;
    await held.event('resumed');
    for (let index = 0; index < FETCHES; index++) {
      const url = `${origin}/api/${index}`;
      const response = raw.event(
        'Network.responseReceived',
        sessionId,
        (params) => params.response.url === url
      );
      const result = await raw.send(
        'Runtime.evaluate',
        {
          expression: `fetch(${JSON.stringify(url)}, {cache:'no-store'}).then(r=>r.text())`,
          awaitPromise: true,
          returnByValue: true,
        },
        sessionId
      );
      assert.equal(result.result?.value, '{"ok":true}', JSON.stringify(result));
      const params = await response.seen;
      seen.push({
        url,
        details: Boolean(params.response.securityDetails),
        timestamps: params.response.securityDetails?.signedCertificateTimestampList?.length ?? 0,
      });
    }
    const stopped = await harCommand(browser.wsUrl, ['har-stop'], home);
    assert.equal(stopped.code, 0, stopped.stderr);
    assert.equal((await held.ended).code, 0);
    const har = JSON.parse(await readFile(file, 'utf8'));
    const urls = har.log.entries.map((entry) => entry.request.url);
    assert.ok(urls.includes(`${origin}/page`), JSON.stringify(urls));
    for (let index = 0; index < FETCHES; index++)
      assert.ok(urls.includes(`${origin}/api/${index}`), JSON.stringify(urls));
    assert.ok(
      seen.every((item) => item.details),
      JSON.stringify(seen)
    );
  } catch (error) {
    const output = held?.output();
    error.message = [
      error.message,
      `responses: ${JSON.stringify(seen)}`,
      `holder stdout:\n${output?.stdout ?? ''}`,
      `holder stderr:\n${output?.stderr ?? ''}`,
    ].join('\n');
    throw error;
  } finally {
    held?.child.kill('SIGKILL');
    raw?.close();
    await browser?.close();
    await new Promise((resolve) => {
      server.closeAllConnections();
      server.close(resolve);
    });
    await rm(home, { recursive: true, force: true });
  }
});
