import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { launchBrowser, rawSocket } from './browser.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '../..');
const nativeDir = path.join(root, 'target/debug');
const wasmDir = path.join(root, 'target/wasm32-wasip1/debug');
const spec = JSON.parse(await readFile(path.join(here, 'guard-vectors.json'), 'utf8'));
const settleMs = 1500;
const cliWaitMs = 6000;
const fetchWaitMs = 5000;
const tall = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==',
  'base64'
);
const wide =
  'iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAIAAAB7QOjdAAAADUlEQVR4nGP4z8AARAAI/gH/xp559wAAAABJRU5ErkJggg==';
const probes = {
  fetch: {
    path: 'data',
    run: (url) => `fetch(${JSON.stringify(url)}).then((r) => r.text(), () => 'error')`,
    fulfilled: 'fulfilled',
    real: (key) => `real-${key}`,
    type: 'text/plain',
  },
  xhr: {
    path: 'data',
    run: (url) =>
      `new Promise((resolve) => { const x = new XMLHttpRequest(); x.open('GET', ${JSON.stringify(url)}); x.onload = () => resolve(x.responseText); x.onerror = () => resolve('error'); x.send(); })`,
    fulfilled: 'fulfilled',
    real: (key) => `real-${key}`,
    type: 'text/plain',
  },
  image: {
    path: 'img',
    run: (url) =>
      `new Promise((resolve) => { const i = new Image(); i.onload = () => resolve('width ' + i.naturalWidth); i.onerror = () => resolve('error'); i.src = ${JSON.stringify(url)}; })`,
    fulfilled: 'width 2',
    real: () => 'width 1',
    type: 'image/png',
    body: wide,
  },
  script: {
    path: 'script',
    run: (url) =>
      `new Promise((resolve) => { const s = document.createElement('script'); s.onload = () => resolve(String(globalThis.vectorScript)); s.onerror = () => resolve('error'); s.src = ${JSON.stringify(url)}; document.head.append(s); })`,
    fulfilled: 'fulfilled',
    real: (key) => `real-${key}`,
    type: 'text/javascript',
    source: "globalThis.vectorScript = 'fulfilled'",
  },
  auth: {
    path: 'auth',
    run: (url) =>
      `fetch(${JSON.stringify(url)}, { credentials: 'include' }).then(async (r) => r.status + ' ' + (await r.text()), () => 'error')`,
  },
};

test('guard vectors through the raw client, playwright-cli and the kernel', async (t) => {
  const site = await startFixture();
  let browser;
  let kernel;
  const rows = [];
  try {
    browser = await launchBrowser([
      `--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1:${site.port}, MAP vault.sliccy.ai 127.0.0.1:${site.port}`,
      '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
    ]);
    const ctx = {
      site,
      wsUrl: browser.wsUrl,
      ordinary: site.origin,
      alt: `http://localhost:${site.port}`,
    };
    await plant(ctx);
    rows.push(...(await rawLane(ctx)));
    process.stdout.write(table(rows.filter((row) => row.id.startsWith('C5.'))));
    rows.push(...(await verifyStorage(ctx)));
    const native = nativeRunner(
      path.join(nativeDir, process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'),
      path.join(nativeDir, process.platform === 'win32' ? 'curlwright.exe' : 'curlwright'),
      browser.wsUrl
    );
    rows.push(...(await cliLane(ctx, native, 'cli', 6)));
    kernel = await kernelRunner(
      path.join(wasmDir, 'playwright-cli.wasm'),
      path.join(wasmDir, 'curlwright.wasm'),
      browser.wsUrl
    );
    rows.push(...(await cliLane(ctx, kernel, 'kernel', 1)));
    const report = table(rows);
    const file = process.env.SLICC_GUARD_REPORT || path.join(tmpdir(), 'guard-vectors-report.md');
    await writeFile(file, report);
    process.stdout.write(`${report}\nreport: ${file}\n`);
    for (const row of rows) {
      await t.test(`${row.lane} ${row.id} ${row.target} ${row.variant}`, () => {
        assert.notEqual(row.control, 'fail', `control: ${row.why}`);
        assert.notEqual(row.guarded, 'fail', `guarded: ${row.why}`);
      });
    }
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

function startFixture() {
  const log = [];
  const server = createServer((req, res) => {
    const url = new URL(req.url || '/', 'http://fixture');
    const host = (req.headers.host || '').toLowerCase();
    const q = Object.fromEntries(url.searchParams);
    log.push({ host, path: url.pathname, q, probe: req.headers['x-probe'] || null });
    answer(req, res, url, host, q);
  });
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      const port = typeof address === 'object' && address ? address.port : 0;
      resolve({
        port,
        origin: `http://127.0.0.1:${port}`,
        log,
        close: () =>
          new Promise((done) => {
            server.closeAllConnections();
            server.close(() => done());
          }),
      });
    });
  });
}

function answer(req, res, url, host, q) {
  const html = (body, headers = {}) => {
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', ...headers });
    res.end(body);
  };
  if (url.pathname.startsWith('/canary/')) {
    return html(
      `<!doctype html><title>canary</title><p>canary-marker ${host}</p><script>if (!localStorage.getItem('canary')) localStorage.setItem('canary', 'planted')</script>`,
      q.csp ? { 'content-security-policy': "img-src 'none'" } : {}
    );
  }
  const cors = () => {
    const origin = req.headers.origin;
    return origin
      ? { 'access-control-allow-origin': origin, 'access-control-allow-credentials': 'true' }
      : { 'access-control-allow-origin': '*' };
  };
  switch (url.pathname) {
    case '/data':
      res.writeHead(200, { 'content-type': 'text/plain', ...cors() });
      return res.end(`real-${q.n || ''}`);
    case '/img':
      res.writeHead(200, { 'content-type': 'image/png', ...cors() });
      return res.end(tall);
    case '/script':
      res.writeHead(200, { 'content-type': 'text/javascript', ...cors() });
      return res.end(`globalThis.vectorScript = ${JSON.stringify(`real-${q.n || ''}`)}`);
    case '/auth': {
      const header = req.headers.authorization || '';
      if (header === `Basic ${Buffer.from('u:p').toString('base64')}`) {
        res.writeHead(200, { 'content-type': 'text/plain', ...cors() });
        return res.end('authed');
      }
      res.writeHead(401, {
        'content-type': 'text/plain',
        'www-authenticate': 'Basic realm="vector"',
        ...cors(),
      });
      return res.end('unauthorized');
    }
    case '/beacon':
      res.writeHead(204, { 'access-control-allow-origin': '*' });
      return res.end();
    case '/page':
      return html(triggerPage(q.k || '', q.to || ''));
    case '/hop': {
      const n = Number(q.n || '0');
      const next =
        n > 1 ? `/hop?n=${n - 1}&code=${q.code}&to=${encodeURIComponent(q.to)}` : q.to || '/';
      res.writeHead(Number(q.code || '302'), { location: next, 'content-type': 'text/plain' });
      return res.end('moved');
    }
    case '/plant':
      return html(
        `<!doctype html><title>plant</title><p>planted</p><script>localStorage.setItem('canary', 'planted')</script>`,
        {
          'set-cookie': [`canary=planted-${host.split(':')[0]}; Path=/`, 'del=1; Path=/'],
        }
      );
    case '/rewritten':
      return html('<!doctype html><title>rewritten</title><p>REWRITTEN</p>');
    case '/api/cookies':
      res.writeHead(200, { 'content-type': 'text/plain' });
      return res.end(req.headers.cookie || 'none');
    default:
      res.writeHead(404, { 'content-type': 'text/plain' });
      return res.end('missing');
  }
}

function triggerPage(kind, to) {
  const frame = kind === 'iframe' ? `<iframe src="${escapeAttr(to)}"></iframe>` : '';
  return `<!doctype html><title>trigger</title><p id="out">trigger</p><button onclick="go()">Go</button>${frame}<script>
const kind = ${JSON.stringify(kind)};
const to = ${JSON.stringify(to)};
function go() {
  if (kind === 'href') location.href = to;
  if (kind === 'replace') location.replace(to);
  if (kind === 'popup') window.open(to);
  if (kind === 'form') {
    const target = new URL(to);
    const form = document.createElement('form');
    form.method = 'GET';
    form.action = target.origin + target.pathname;
    for (const [name, value] of target.searchParams) {
      const input = document.createElement('input');
      input.type = 'hidden';
      input.name = name;
      input.value = value;
      form.append(input);
    }
    document.body.append(form);
    form.submit();
  }
  return 'went';
}
</script>`;
}

function escapeAttr(value) {
  return value.replace(/&/g, '&amp;').replace(/"/g, '&quot;');
}

function probeScript(key) {
  return `(() => {
  const k = ${JSON.stringify(key)};
  const o = location.origin;
  const p = location.pathname;
  new Image().src = '/beacon?id=' + encodeURIComponent(k) + '&o=' + encodeURIComponent(o) + '&p=' + encodeURIComponent(p);
  if (window.probeBinding) window.probeBinding(btoa(o + p));
  console.log('probe:' + btoa(o + p));
  try { localStorage.setItem('probe', k); } catch {}
})()`;
}

function urlsFor(ctx, c, key, origin) {
  const dest = `${origin}/canary/${key}${c.csp ? '?csp=1' : ''}`;
  const to = c.hop
    ? `${ctx.ordinary}/hop?n=${c.hop.n}&code=${c.hop.code}&to=${encodeURIComponent(dest)}`
    : dest;
  const page = `${ctx.ordinary}/page?k=${c.trigger}&to=${encodeURIComponent(to)}&case=${key}`;
  return { dest, page };
}

function interceptRule(ctx, c, key) {
  if (c.intercept === 'fulfill') {
    const body = Buffer.from('<!doctype html><title>fulfilled</title><p>FULFILLED</p>').toString(
      'base64'
    );
    return {
      match: '/canary',
      method: 'Fetch.fulfillRequest',
      params: {
        responseCode: 200,
        responseHeaders: [{ name: 'content-type', value: 'text/html' }],
        body,
      },
    };
  }
  if (c.intercept === 'continue-url') {
    return {
      match: '/canary',
      method: 'Fetch.continueRequest',
      params: { url: `${ctx.ordinary}/rewritten?case=${key}` },
    };
  }
  return { match: '/canary', method: 'Fetch.continueRequest', params: {} };
}

function setupSteps(ctx, c, key) {
  const steps = [];
  for (const item of c.setup) {
    if (item === 'probe') {
      steps.push(call('Page.addScriptToEvaluateOnNewDocument', { source: probeScript(key) }));
    }
    if (item === 'onload') {
      steps.push(call('Page.addScriptToEvaluateOnLoad', { scriptSource: probeScript(key) }));
    }
    if (item === 'autoattach') {
      steps.push(
        call(
          'Target.setAutoAttach',
          { autoAttach: true, flatten: true, waitForDebuggerOnStart: true },
          null
        )
      );
    }
    if (item === 'binding') {
      steps.push(call('Runtime.enable', {}));
      steps.push(call('Runtime.addBinding', { name: 'probeBinding' }));
    }
    if (item === 'bypasscsp') steps.push(call('Page.setBypassCSP', { enabled: true }));
    if (item === 'console') steps.push(call('Runtime.enable', {}));
    if (item === 'headers') {
      steps.push(call('Network.enable', {}));
      steps.push(call('Network.setExtraHTTPHeaders', { headers: { 'x-probe': key } }));
    }
    if (item === 'fetch') {
      steps.push(call('Fetch.enable', { patterns: [{ urlPattern: '*' }] }));
      steps.push({ op: 'intercept', rule: interceptRule(ctx, c, key) });
    }
  }
  return steps;
}

function call(method, params, session = 'page') {
  const step = { op: 'call', method, params };
  if (session) step.session = session;
  return step;
}

function navigationRun(ctx, c, side, variant) {
  const origin = side === 'control' ? ctx.ordinary : spec.targets[side];
  const key = `${c.id}.${side}.${variant}.raw`;
  const { dest, page } = urlsFor(ctx, c, key, origin);
  const steps = [
    { op: 'open' },
    call('Page.enable', {}),
    ...setupSteps(ctx, c, key),
    { op: 'call', method: 'Page.navigate', params: { url: page }, session: 'page', notify: true },
    { op: 'wait', ms: settleMs },
    { op: 'eval', expression: 'go()' },
    { op: 'wait', ms: spec.variants[variant] },
    { op: 'eval', expression: "document.body ? document.body.innerText : ''" },
  ];
  return { key, steps, meta: { c, side, variant, origin, dest } };
}

function cookieRun(ctx, c, side) {
  const origin =
    side === 'control' ? (c.control === 'alt' ? ctx.alt : ctx.ordinary) : spec.targets[side];
  const key = `${c.id}.${side}.raw`;
  const params = JSON.parse(
    JSON.stringify(c.params)
      .replaceAll('$target', origin)
      .replaceAll('$ordinary', ctx.ordinary)
      .replaceAll('$key', key)
  );
  const steps = [
    { op: 'open' },
    call('Page.enable', {}),
    {
      op: 'call',
      method: 'Page.navigate',
      params: { url: `${ctx.ordinary}/page?k=none&case=${key}` },
      session: 'page',
      notify: true,
    },
    { op: 'wait', ms: settleMs },
    call(c.method, params, c.session || null),
  ];
  return { key, steps, meta: { c, side, variant: '-', origin } };
}

function bounded(expression, ms = 6000) {
  return `Promise.race([${expression}, new Promise((resolve) => setTimeout(() => resolve('timeout'), ${ms}))])`;
}

function fetchUrls(ctx, c, origin, key) {
  const probe = probes[c.probe];
  return {
    url: `${origin}/${probe.path}?n=${encodeURIComponent(key)}`,
    page: `${ctx.ordinary}/page?k=none&case=${key}`,
  };
}

const corsList = (type) => [
  { name: 'content-type', value: type },
  { name: 'access-control-allow-origin', value: '*' },
];

function fetchRun(ctx, c, side) {
  const origin = side === 'control' ? ctx.ordinary : spec.targets[side];
  const key = `${c.id}.${side}.raw`;
  const probe = probes[c.probe];
  const { url, page } = fetchUrls(ctx, c, origin, key);
  const match = `/${probe.path}?n=${encodeURIComponent(key)}`;
  const steps = [
    { op: 'open' },
    call('Page.enable', {}),
    { op: 'call', method: 'Page.navigate', params: { url: page }, session: 'page', notify: true },
    { op: 'wait', ms: settleMs },
  ];
  const bracket = (name) => `${ctx.ordinary}/data?n=${encodeURIComponent(`${name}.${key}`)}`;
  const launch = (expression, slot, ms) => ({
    op: 'launch',
    slot,
    expression: bounded(expression, ms),
  });
  if (c.expect === 'subresource') {
    const body = probe.body || Buffer.from(probe.source || probe.fulfilled).toString('base64');
    steps.push(
      call('Fetch.enable', {
        patterns: [
          { urlPattern: '*', requestStage: 'Request' },
          { urlPattern: '*', requestStage: 'Response' },
        ],
      }),
      {
        op: 'intercept',
        rule: {
          match,
          stage: 'Request',
          method: 'Fetch.fulfillRequest',
          params: { responseCode: 200, responseHeaders: corsList(probe.type), body },
        },
      },
      launch(probe.run(url), 'probe'),
      { op: 'wait', ms: fetchWaitMs },
      { op: 'read', slot: 'probe' }
    );
  }
  if (c.expect === 'auth') {
    steps.push(
      launch(probe.run(`${url}&plain=1`), 'plain'),
      { op: 'wait', ms: fetchWaitMs },
      { op: 'read', slot: 'plain' },
      call('Fetch.enable', { patterns: [{ urlPattern: '*' }], handleAuthRequests: true }),
      {
        op: 'intercept',
        rule: { match: '/never-matches', method: 'Fetch.continueRequest', params: {} },
        auth: {
          match,
          response: { response: 'ProvideCredentials', username: 'u', password: 'p' },
        },
      },
      launch(probes.fetch.run(bracket('before')), 'before'),
      { op: 'wait', ms: 500 },
      launch(probe.run(url), 'probe'),
      { op: 'wait', ms: fetchWaitMs },
      launch(probes.fetch.run(bracket('after')), 'after'),
      { op: 'wait', ms: 500 },
      { op: 'guess', before: `before.${key}`, after: `after.${key}`, calls: c.calls },
      { op: 'read', slot: 'probe' }
    );
  }
  if (c.expect === 'guessed') {
    steps.push(
      call('Fetch.enable', { patterns: [{ urlPattern: '*', requestStage: 'Request' }] }),
      {
        op: 'intercept',
        rule: { match, method: 'Fetch.continueRequest', params: {}, hold: true },
      },
      launch(probes.fetch.run(bracket('before')), 'before'),
      { op: 'wait', ms: 500 },
      launch(probe.run(url), 'probe', 20000),
      { op: 'wait', ms: 500 },
      launch(probes.fetch.run(bracket('after')), 'after'),
      { op: 'wait', ms: 500 },
      { op: 'guess', before: `before.${key}`, after: `after.${key}`, calls: c.calls },
      { op: 'wait', ms: fetchWaitMs },
      { op: 'read', slot: 'probe' }
    );
  }
  return { key, steps, meta: { c, side, variant: '-', origin, url } };
}

function readValue(out, slot) {
  const read = (out?.steps || []).filter((step) => step.op === 'read' && step.slot === slot);
  return read.at(-1)?.outcome?.result?.result?.value;
}

function isRefused(outcome) {
  return !outcome.ok && String(outcome.error).startsWith('slicc: refused');
}

function judgeFetch(ctx, run, out) {
  const { c, side, origin, url } = run.meta;
  if (!out) return { ok: false, why: 'no output from the raw client' };
  const opened = stepOf(out, 'open')?.outcome;
  if (!opened?.ok) return { ok: false, why: `setup: ${opened?.error?.trim()}` };
  const probe = probes[c.probe];
  const key = run.key;
  const host = new URL(origin).host;
  const value = readValue(out, 'probe');
  const paused = (out.paused || []).filter((item) => item.url === url);
  const events = out.events || [];
  const challenged = events.filter(
    (event) => event.method === 'Fetch.authRequired' && event.params?.request?.url === url
  );
  const hits = ctx.site.log.filter(
    (hit) => hit.host === host && hit.path === `/${probe.path}` && hit.q.n === key
  );
  const guess = stepOf(out, 'guess')?.outcome;
  const attempts = guess?.attempts || [];
  const failed = out.steps.find(
    (step) => step.op === 'call' && !step.outcome.ok && step.method !== 'Page.navigate'
  );
  const problems = [];
  const want = (ok, text) => {
    if (!ok) problems.push(text);
  };
  if (failed) problems.push(`${failed.method}: ${String(failed.outcome.error).trim()}`);
  if (c.expect === 'subresource') {
    if (side === 'control') {
      want(paused.length > 0, 'no requestPaused for the ordinary url');
      want(value === probe.fulfilled, `the ordinary probe read ${value}, not the fulfilled body`);
    } else {
      want(paused.length === 0, 'requestPaused for the guarded url');
      want(value === probe.real(key), `the guarded probe read ${value}, not the real body`);
      want(hits.length > 0, 'the guarded server never answered the real request');
    }
  }
  if (c.expect === 'auth') {
    const plain = readValue(out, 'plain');
    if (side === 'control') {
      want(challenged.length > 0, 'no authRequired for the ordinary url');
      want(paused.length > 0, 'no requestPaused for the ordinary url');
      want(value === '200 authed', `the ordinary probe read ${value}, not 200 authed`);
    } else {
      want(challenged.length === 0, 'authRequired for the guarded url');
      want(paused.length === 0, 'requestPaused for the guarded url');
      want(plain !== undefined && plain !== '200 authed', `the plain probe read ${plain}`);
      want(value === plain, `the guarded probe read ${value}, not the untouched ${plain}`);
      want(attempts.length > 0, 'no candidate ids to continue with auth');
      want(
        attempts.every((item) => isRefused(item.outcome)),
        'continueWithAuth was not refused'
      );
    }
  }
  if (c.expect === 'guessed') {
    want(attempts.length > 0, `no candidate ids${guess?.error ? `: ${guess.error}` : ''}`);
    if (side === 'control') {
      want(value === 'guessed', `the ordinary probe read ${value}, not the guessed body`);
    } else {
      const passed = attempts.filter((item) => !isRefused(item.outcome));
      want(
        passed.length === 0,
        `not refused: ${[...new Set(passed.map((item) => item.method))].join(', ')}`
      );
      want(value === `real-${key}`, `the guarded probe read ${value}, not the real body`);
      want(hits.length > 0, 'the guarded server never answered the real request');
    }
  }
  if (side !== 'control') {
    const text = JSON.stringify(events);
    if (text.includes(`${host}/${probe.path}`)) problems.push('an event carried the guarded url');
  }
  return problems.length ? { ok: false, why: problems.join(', ') } : { ok: true };
}

async function rawLane(ctx) {
  const runs = [];
  for (const c of spec.fetch.filter((item) => item.lanes.includes('raw'))) {
    for (const side of ['control', ...Object.keys(spec.targets)]) {
      runs.push(fetchRun(ctx, c, side));
    }
  }
  for (const c of spec.navigation.filter((item) => item.lanes.includes('raw'))) {
    for (const variant of Object.keys(spec.variants)) {
      for (const side of ['control', ...Object.keys(spec.targets)]) {
        runs.push(navigationRun(ctx, c, side, variant));
      }
    }
  }
  for (const c of spec.cookies.filter((item) => item.lanes.includes('raw'))) {
    for (const side of ['control', ...Object.keys(spec.targets)])
      runs.push(cookieRun(ctx, c, side));
  }
  const dir = await mkdtemp(path.join(tmpdir(), 'slicc-guard-vectors-'));
  const planFile = path.join(dir, 'plan.json');
  const outFile = path.join(dir, 'out.json');
  await writeFile(
    planFile,
    JSON.stringify({
      ws: ctx.wsUrl,
      workers: 6,
      runs: runs.map(({ key, steps }) => ({ key, steps })),
    })
  );
  const result = await cargo(
    [
      'test',
      '-p',
      'slicc-cdp-client',
      '--lib',
      '--',
      '--ignored',
      '--exact',
      'vectors::live_guard_vectors',
      '--test-threads=1',
    ],
    { SLICC_GUARD_PLAN: planFile, SLICC_GUARD_OUT: outFile }
  );
  assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
  await delay(1000);
  const outputs = new Map(
    JSON.parse(await readFile(outFile, 'utf8')).map((entry) => [entry.key, entry])
  );
  const grouped = new Map();
  for (const run of runs) {
    const { c, side, variant } = run.meta;
    const out = outputs.get(run.key);
    const judged = c.probe
      ? judgeFetch(ctx, run, out)
      : c.method
        ? judgeCookie(ctx, run, out)
        : judgeNavigation(ctx, run, out);
    const group = `${c.id}|${variant}`;
    if (!grouped.has(group)) grouped.set(group, { control: null, guarded: [] });
    if (side === 'control') grouped.get(group).control = judged;
    else grouped.get(group).guarded.push({ side, judged });
  }
  const rows = [];
  for (const [group, { control, guarded }] of grouped) {
    const [id, variant] = group.split('|');
    for (const { side, judged } of guarded) {
      rows.push({
        lane: 'raw',
        id,
        target: side,
        variant,
        control: control.ok ? 'pass' : 'fail',
        guarded: judged.ok ? 'pass' : 'fail',
        why: [control.ok ? '' : `control ${control.why}`, judged.ok ? '' : judged.why]
          .filter(Boolean)
          .join('; '),
      });
    }
  }
  return rows;
}

function stepOf(out, method) {
  return out?.steps?.find((step) => step.method === method || step.op === method);
}

function lastEval(out) {
  const evals = (out?.steps || []).filter((step) => step.op === 'eval');
  return evals.at(-1)?.outcome;
}

function beacons(ctx, key) {
  return ctx.site.log.filter((hit) => hit.path === '/beacon' && hit.q.id === key);
}

function served(ctx, key, host) {
  return ctx.site.log.filter((hit) => hit.path === `/canary/${key}` && hit.host === host);
}

function judgeNavigation(ctx, run, out) {
  const { c, side, origin } = run.meta;
  if (!out) return { ok: false, why: 'no output from the raw client' };
  const opened = stepOf(out, 'open')?.outcome;
  if (!opened?.ok) return { ok: false, why: `setup: ${opened?.error?.trim()}` };
  const host = new URL(origin).host;
  const key = run.key;
  const fromTarget = beacons(ctx, key).filter(
    (hit) => hit.q.o === origin && hit.q.p === `/canary/${key}`
  );
  const events = out.events || [];
  const named = (method) => events.filter((event) => event.method === method);
  const encoded = Buffer.from(`${origin}/canary/${key}`).toString('base64');
  const pausedTarget = (out.paused || []).filter((item) => item.url.includes(`${host}/canary`));
  const others = named('Target.attachedToTarget').filter(
    (event) => event.params?.targetInfo?.targetId !== out.target
  );
  const body = out.observed?.page?.body || '';
  const evalBody = lastEval(out)?.result?.result?.value || '';
  const expectedRefusals = c.refusedAtRegistration || [];
  const registered = out.steps.filter((step) => expectedRefusals.includes(step.method));
  if (
    registered.some((step) => step.outcome.ok || !step.outcome.error.startsWith('slicc: refused'))
  ) {
    return { ok: false, why: `${expectedRefusals.join(', ')} was not refused at registration` };
  }
  if (side === 'control') {
    const setupFailed = out.steps.find(
      (step) => step.op === 'call' && !step.outcome.ok && !expectedRefusals.includes(step.method)
    );
    const why = (text) => ({
      ok: false,
      why: `${text}${setupFailed ? ` (${setupFailed.method}: ${setupFailed.outcome.error.trim()})` : ''}`,
    });
    switch (c.expect) {
      case 'popup':
        if (expectedRefusals.length) {
          return (out.observed?.popups || []).some((popup) => popup.url === run.meta.dest)
            ? { ok: true }
            : why('the popup never opened on the ordinary target');
        }
        return fromTarget.length ? { ok: true } : why('no beacon from the ordinary target');
      case 'beacon':
      case 'iframe':
        return fromTarget.length ? { ok: true } : why('no beacon from the ordinary target');
      case 'binding':
        return named('Runtime.bindingCalled').some((event) => event.params?.payload === encoded)
          ? { ok: true }
          : why('no bindingCalled from the ordinary target');
      case 'console':
        return named('Runtime.consoleAPICalled').some(
          (event) => event.params?.args?.[0]?.value === `probe:${encoded}`
        )
          ? { ok: true }
          : why('no consoleAPICalled from the ordinary target');
      case 'header':
        return served(ctx, key, host).some((hit) => hit.probe === key)
          ? { ok: true }
          : why('the ordinary target request had no extra header');
      case 'fulfill':
        return pausedTarget.some((item) => item.answer?.ok) && evalBody.includes('FULFILLED')
          ? { ok: true }
          : why('fulfillRequest did not render');
      case 'rewrite':
        return pausedTarget.some((item) => item.answer?.ok) && evalBody.includes('REWRITTEN')
          ? { ok: true }
          : why('continueRequest with url did not render');
      case 'paused':
        return pausedTarget.length ? { ok: true } : why('no requestPaused for the target');
      default:
        return why(`unknown expectation ${c.expect}`);
    }
  }
  const problems = [];
  const leaked = beacons(ctx, key).filter((hit) => hit.q.o === origin);
  if (leaked.length) problems.push(`beacon from ${origin}`);
  const text = JSON.stringify(events);
  if (text.includes('canary-marker') || text.includes(`${host}/canary`)) {
    problems.push('an event carried the guarded document');
  }
  if (!['popup', 'iframe'].includes(c.expect) && served(ctx, key, host).length) {
    problems.push('the server served the guarded page');
  }
  if (others.length) problems.push('attachedToTarget for another target');
  if (named('Runtime.bindingCalled').some((event) => event.params?.payload === encoded)) {
    problems.push('bindingCalled from the guarded document');
  }
  if (
    named('Runtime.consoleAPICalled').some(
      (event) => event.params?.args?.[0]?.value === `probe:${encoded}`
    )
  ) {
    problems.push('consoleAPICalled from the guarded document');
  }
  if (served(ctx, key, host).some((hit) => hit.probe)) {
    problems.push('the guarded request carried the extra header');
  }
  if (pausedTarget.length) problems.push('requestPaused for the guarded url');
  if (
    named('Fetch.authRequired').some((event) => JSON.stringify(event).includes(`${host}/canary`)) ||
    named('Fetch.requestPaused').length
  ) {
    problems.push('Fetch event reached the client');
  }
  if (body.includes('FULFILLED') || body.includes('REWRITTEN')) {
    problems.push('the guarded page shows the client body');
  }
  return problems.length ? { ok: false, why: problems.join(', ') } : { ok: true };
}

function cookieHosts(result) {
  return (result?.cookies || []).map((cookie) => `${cookie.domain}|${cookie.name}`);
}

function judgeCookie(ctx, run, out) {
  const { c, side, origin } = run.meta;
  if (!out) return { ok: false, why: 'no output from the raw client' };
  const opened = stepOf(out, 'open')?.outcome;
  if (!opened?.ok) return { ok: false, why: `setup: ${opened?.error?.trim()}` };
  const outcome = stepOf(out, c.method)?.outcome;
  if (!outcome) return { ok: false, why: 'the call never ran' };
  const host = new URL(origin).hostname;
  if (side === 'control') {
    if (!outcome.ok) return { ok: false, why: outcome.error.trim() };
    if (c.expect === 'read') {
      return cookieHosts(outcome.result).includes(`${host}|canary`)
        ? { ok: true }
        : { ok: false, why: 'the ordinary cookie was not read' };
    }
    return { ok: true };
  }
  if (c.expect === 'read') {
    if (!outcome.ok) {
      return outcome.error.startsWith('slicc: refused')
        ? { ok: true }
        : { ok: false, why: outcome.error.trim() };
    }
    const hosts = cookieHosts(outcome.result);
    if (hosts.some((item) => item.startsWith(`${host}|`))) {
      return { ok: false, why: 'a guarded cookie was returned' };
    }
    return hosts.includes(`127.0.0.1|canary`)
      ? { ok: true }
      : { ok: false, why: 'the ordinary cookie went missing from the result' };
  }
  if (outcome.ok) return { ok: false, why: `${c.method} was allowed` };
  return outcome.error.startsWith('slicc: refused')
    ? { ok: true }
    : { ok: false, why: outcome.error.trim() };
}

async function plant(ctx) {
  const raw = await rawSocket(ctx.wsUrl);
  try {
    for (const origin of [ctx.ordinary, ctx.alt, ...Object.values(spec.targets)]) {
      const tab = await raw.open(`${origin}/plant`);
      await delay(300);
      await raw.send('Target.closeTarget', { targetId: tab });
    }
  } finally {
    raw.close();
  }
}

async function readStorage(raw, origin) {
  const tab = await raw.open(`${origin}/canary/final`);
  try {
    const { sessionId } = await raw.send('Target.attachToTarget', { targetId: tab, flatten: true });
    const expression = `location.href.startsWith(${JSON.stringify(origin)}) && document.readyState === 'complete' ? JSON.stringify({ canary: localStorage.getItem('canary'), probe: localStorage.getItem('probe') }) : ''`;
    const deadline = Date.now() + 10000;
    for (;;) {
      const value = await raw
        .send('Runtime.evaluate', { expression, returnByValue: true }, sessionId)
        .catch(() => null);
      const text = value?.result?.value;
      if (text) return JSON.parse(text);
      if (Date.now() > deadline) throw new Error(`${origin}/canary/final never loaded`);
      await delay(200);
    }
  } finally {
    await raw.send('Target.closeTarget', { targetId: tab });
  }
}

async function verifyStorage(ctx) {
  const raw = await rawSocket(ctx.wsUrl);
  const rows = [];
  try {
    const { cookies } = await raw.send('Storage.getCookies', {});
    const on = (host) => cookies.filter((cookie) => cookie.domain === host);
    const ordinary = on('127.0.0.1');
    const alt = on('localhost');
    const controls = {
      'C6.setCookie': ordinary.some(
        (cookie) => cookie.name === 'probe' && cookie.value === 'C6.setCookie.control.raw'
      ),
      'C6.deleteCookies': !ordinary.some((cookie) => cookie.name === 'del'),
      'C6.clearDataForOrigin': alt.length === 0,
    };
    for (const [name, url] of Object.entries(spec.targets)) {
      const host = new URL(url).hostname;
      const mine = on(host);
      const storage = await readStorage(raw, url);
      const problems = [];
      if (!mine.some((cookie) => cookie.name === 'canary' && cookie.value === `planted-${host}`)) {
        problems.push('canary cookie changed');
      }
      if (!mine.some((cookie) => cookie.name === 'del')) problems.push('del cookie removed');
      if (mine.some((cookie) => cookie.name === 'probe')) problems.push('probe cookie written');
      if (storage.canary !== 'planted') problems.push('canary localStorage changed');
      if (storage.probe) problems.push(`probe localStorage written by ${storage.probe}`);
      for (const [id, ok] of Object.entries(controls)) {
        rows.push({
          lane: 'raw-state',
          id,
          target: name,
          variant: '-',
          control: ok ? 'pass' : 'fail',
          guarded: problems.length ? 'fail' : 'pass',
          why: [ok ? '' : 'control write had no effect', ...problems].filter(Boolean).join(', '),
        });
      }
    }
  } finally {
    raw.close();
  }
  return rows;
}

async function cliLane(ctx, runner, lane, width) {
  const jobs = [];
  for (const c of [...spec.fetch, ...spec.navigation, ...spec.cookies].filter((item) =>
    item.lanes.includes(lane)
  )) {
    jobs.push(async () => {
      const control = await cliCase(ctx, runner, lane, c, 'control');
      const results = [];
      for (const side of Object.keys(spec.targets)) {
        const guarded = await cliCase(ctx, runner, lane, c, side);
        results.push({
          lane: lane === 'cli' ? 'playwright-cli' : 'kernel',
          id: c.id,
          target: side,
          variant: 'prompt',
          control: control.ok ? 'pass' : 'fail',
          guarded: guarded.ok ? 'pass' : 'fail',
          why: [control.ok ? '' : `control ${control.why}`, guarded.ok ? '' : guarded.why]
            .filter(Boolean)
            .join('; '),
        });
      }
      return results;
    });
  }
  const rows = [];
  for (let index = 0; index < jobs.length; index += width) {
    const batch = await Promise.all(jobs.slice(index, index + width).map((job) => job()));
    for (const results of batch) rows.push(...results);
  }
  return rows;
}

async function cliCase(ctx, runner, lane, c, side) {
  const origin = side === 'control' ? ctx.ordinary : spec.targets[side];
  const host = new URL(origin).host;
  const key = `${c.id}.${side}.${lane}`;
  const cwd = await runner.fresh();
  const outputs = [];
  const run = async (args) => {
    const result = await runner.playwright(args, cwd);
    outputs.push(result.stdout + result.stderr);
    return result;
  };
  const raw = await rawSocket(ctx.wsUrl);
  let tab;
  try {
    if (c.probe) return await cliFetch(ctx, runner, c, side, key, cwd, run, (id) => (tab = id));
    if (c.method) {
      const start = `${ctx.ordinary}/page?k=none&case=${key}`;
      let opened = await run(['open', start]);
      if (opened.code !== 0) opened = await run(['open', start]);
      if (opened.code !== 0) return { ok: false, why: `setup: ${opened.stderr.trim()}` };
      tab = targetOf(opened.stdout);
      const curled = await runner.curl(['--tab', tab, `${origin}/api/cookies`], cwd);
      if (side === 'control') {
        return curled.code === 0 && curled.stdout.includes('canary=planted')
          ? { ok: true }
          : { ok: false, why: `curlwright: ${curled.code} ${curled.stderr.trim()}` };
      }
      return curled.code !== 0 &&
        curled.stderr.includes('slicc: refused') &&
        !curled.stdout.includes('planted')
        ? { ok: true }
        : { ok: false, why: `curlwright read the guarded cookie: ${curled.stdout}` };
    }
    if (c.intercept === 'fulfill') {
      const routed = await run([
        'route',
        '**/canary/**',
        '--body=<!doctype html><p>FULFILLED</p>',
        '--content-type=text/html',
      ]);
      if (routed.code !== 0) return { ok: false, why: routed.stderr.trim() };
    }
    const { dest, page } = urlsFor(ctx, c, key, origin);
    let opened = await run(['open', page]);
    if (opened.code !== 0) opened = await run(['open', page]);
    if (opened.code !== 0) return { ok: false, why: `setup: ${opened.stderr.trim()}` };
    tab = targetOf(opened.stdout);
    if (c.intercept === 'fulfill') {
      await run(['goto', `${ctx.ordinary}/hop?n=1&code=302&to=${encodeURIComponent(dest)}`]);
    } else if (c.expect === 'popup') {
      const shot = await run(['snapshot']);
      const ref = shot.stdout.match(/button "Go" \[ref=(e\d+)\]/)?.[1];
      if (!ref) return { ok: false, why: `no Go button: ${shot.stdout}${shot.stderr}` };
      await run(['click', ref]);
    } else {
      await run(['eval', 'go()']);
    }
    const deadline = Date.now() + cliWaitMs;
    let reached = false;
    while (Date.now() < deadline) {
      if (c.expect === 'popup') {
        const listed = await run(['tab-list']);
        if (listed.stdout.includes(dest)) reached = true;
      } else if (c.expect === 'iframe') {
        const frames = await run(['frames']);
        if (frames.stdout.includes(dest)) reached = true;
      } else {
        const probe = await run(['eval', "location.href + ' ' + document.body.innerText"]);
        if (
          c.expect === 'fulfill'
            ? probe.stdout.includes('FULFILLED')
            : probe.stdout.includes(`canary-marker ${host}`)
        ) {
          reached = true;
        }
      }
      if (reached && side === 'control') break;
      await delay(500);
    }
    if (c.intercept === 'fulfill') await run(['unroute']);
    if (side === 'control') {
      return reached
        ? { ok: true }
        : { ok: false, why: `never reached ${dest}: ${outputs.at(-1)?.trim().slice(0, 300)}` };
    }
    const problems = [];
    const text = outputs.join('\n');
    if (reached) problems.push('the client reached the guarded document');
    if (text.includes(`canary-marker ${host}`)) problems.push('output carried the guarded marker');
    if (text.includes(dest)) problems.push('output carried the guarded url');
    if (c.expect === 'fulfill') {
      const body = await rawBody(raw, tab);
      if (body.includes('FULFILLED')) problems.push('the guarded page shows the routed body');
    }
    return problems.length ? { ok: false, why: problems.join(', ') } : { ok: true };
  } finally {
    if (tab) await closeWithPopups(raw, tab);
    raw.close();
  }
}

async function cliFetch(ctx, runner, c, side, key, cwd, run, opened) {
  const origin = side === 'control' ? ctx.ordinary : spec.targets[side];
  const host = new URL(origin).host;
  const probe = probes[c.probe];
  const { url, page } = fetchUrls(ctx, c, origin, key);
  let started = await run(['open', page]);
  if (started.code !== 0) started = await run(['open', page]);
  if (started.code !== 0) return { ok: false, why: `setup: ${started.stderr.trim()}` };
  opened(targetOf(started.stdout));
  const args = ['route', `**/${probe.path}*`, `--content-type=${probe.type}`];
  args.push('--header=access-control-allow-origin: *');
  if (probe.body) {
    await runner.write(cwd, 'fulfilled.png', Buffer.from(probe.body, 'base64'));
    args.push(`--fulfill-file=${cwd}/fulfilled.png`);
  } else {
    args.push(`--body=${probe.source || probe.fulfilled}`);
  }
  const routed = await run(args);
  if (routed.code !== 0) return { ok: false, why: `route: ${routed.stderr.trim()}` };
  const result = await run(['eval', bounded(probe.run(url))]);
  await run(['unroute']);
  const text = `${result.stdout}${result.stderr}`;
  const hits = ctx.site.log.filter(
    (hit) => hit.host === host && hit.path === `/${probe.path}` && hit.q.n === key
  );
  const fulfilled = text.includes(probe.fulfilled);
  const real = text.includes(probe.real(key));
  if (side === 'control') {
    return fulfilled && !real
      ? { ok: true }
      : { ok: false, why: `the routed body never reached the probe: ${text.trim().slice(0, 300)}` };
  }
  const problems = [];
  if (fulfilled) problems.push('the guarded probe read the routed body');
  if (!real) problems.push(`the guarded probe missed the real body: ${text.trim().slice(0, 300)}`);
  if (!hits.length) problems.push('the guarded server never answered the real request');
  return problems.length ? { ok: false, why: problems.join(', ') } : { ok: true };
}

async function rawBody(raw, tab) {
  try {
    const { sessionId } = await raw.send('Target.attachToTarget', { targetId: tab, flatten: true });
    const value = await raw.send(
      'Runtime.evaluate',
      { expression: 'document.body ? document.body.innerText : ""', returnByValue: true },
      sessionId
    );
    return String(value.result.value || '');
  } catch {
    return '';
  }
}

async function closeWithPopups(raw, tab) {
  try {
    const { targetInfos } = await raw.send('Target.getTargets', {});
    for (const info of targetInfos) {
      if (info.openerId === tab || info.targetId === tab) {
        await raw.send('Target.closeTarget', { targetId: info.targetId });
      }
    }
  } catch {
    return undefined;
  }
}

function targetOf(stdout) {
  const match = stdout.match(/\[targetId: ([^\]]+)\]/);
  if (!match) throw new Error(stdout);
  return match[1];
}

function table(rows) {
  const lines = [
    '| lane | id | target | variant | control | guarded | detail |',
    '| --- | --- | --- | --- | --- | --- | --- |',
  ];
  for (const row of rows) {
    lines.push(
      `| ${row.lane} | ${row.id} | ${row.target} | ${row.variant} | ${row.control} | ${row.guarded} | ${row.why.replaceAll('|', '/')} |`
    );
  }
  return `${lines.join('\n')}\n`;
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function cargo(args, extra) {
  return new Promise((resolve, reject) => {
    const env = { ...process.env, ...extra };
    delete env.SLICC_CDP_URL;
    const child = spawn('cargo', args, { cwd: root, env });
    const out = [];
    const err = [];
    child.stdout.on('data', (chunk) => out.push(chunk));
    child.stderr.on('data', (chunk) => err.push(chunk));
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      reject(new Error('cargo test timed out'));
    }, 600000);
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
