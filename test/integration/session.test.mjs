import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, realpath, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const nativeDir = path.join(root, 'target/debug');
const cli = path.join(
  nativeDir,
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);

function hostHome() {
  if (process.platform === 'win32' && process.env.USERPROFILE) return process.env.USERPROFILE;
  if (process.env.HOME) return process.env.HOME;
  return process.env.USERPROFILE || '';
}

function hostConfigDir() {
  const home = hostHome();
  if (process.platform === 'darwin') return path.join(home, 'Library', 'Application Support');
  if (process.platform === 'win32') {
    if (process.env.APPDATA) return process.env.APPDATA;
    return path.join(home, 'AppData', 'Roaming');
  }
  const xdg = process.env.XDG_CONFIG_HOME;
  if (xdg?.startsWith('/')) return xdg;
  return path.join(home, '.config');
}

async function sessionFile(cwd, sessionName) {
  const canonical = await realpath(cwd);
  let input;
  if (sessionName) {
    const cwdBuf = Buffer.from(canonical);
    const sep = Buffer.from([0]);
    const nameBuf = Buffer.from(sessionName);
    input = Buffer.concat([cwdBuf, sep, nameBuf]);
  } else {
    input = Buffer.from(canonical);
  }
  const key = createHash('sha256').update(input).digest('hex');
  return path.join(hostConfigDir(), 'slicc', 'playwright-cli', 'sessions', `${key}.json`);
}

function run(args, cwd, env = {}) {
  return new Promise((resolve, reject) => {
    const merged = { ...process.env, ...env };
    delete merged.SLICC_CDP_URL;
    delete merged.PLAYWRIGHT_CLI_SESSION;
    Object.assign(merged, env);
    const child = spawn(cli, args, { cwd, env: merged, stdio: ['ignore', 'pipe', 'pipe'] });
    const out = [];
    const err = [];
    child.stdout.on('data', (chunk) => out.push(chunk));
    child.stderr.on('data', (chunk) => err.push(chunk));
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      reject(new Error(`timed out: ${args.join(' ')}`));
    }, 15000);
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

test('per-agent sessions: two sessions in one cwd each eval in their own tab', {
  timeout: 60_000,
}, async () => {
  const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-session-'));
  const cleanups = [cwd];
  let browser;
  try {
    browser = await launchBrowser();
    const wsUrl = browser.wsUrl;

    const sessA = await sessionFile(cwd, 'agent-a');
    const sessB = await sessionFile(cwd, 'agent-b');
    cleanups.push(sessA, sessB);

    const openA = await run(['--cdp', wsUrl, '-s=agent-a', 'open', 'about:blank'], cwd);
    assert.equal(openA.code, 0, `open A failed: ${openA.stderr}`);

    const openB = await run(['--cdp', wsUrl, '--session=agent-b', 'open', 'about:blank'], cwd);
    assert.equal(openB.code, 0, `open B failed: ${openB.stderr}`);

    const evalA = await run(
      ['--cdp', wsUrl, '-s=agent-a', 'eval', 'document.title = "page-a"; document.title'],
      cwd
    );
    assert.equal(evalA.code, 0, `eval A failed: ${evalA.stderr}`);
    assert.ok(evalA.stdout.includes('page-a'), `eval A output: ${evalA.stdout}`);

    const evalB = await run(
      ['--cdp', wsUrl, '--session=agent-b', 'eval', 'document.title = "page-b"; document.title'],
      cwd
    );
    assert.equal(evalB.code, 0, `eval B failed: ${evalB.stderr}`);
    assert.ok(evalB.stdout.includes('page-b'), `eval B output: ${evalB.stdout}`);

    const fileA = JSON.parse(await readFile(sessA, 'utf8'));
    const fileB = JSON.parse(await readFile(sessB, 'utf8'));
    assert.equal(fileA.sessionName, 'agent-a');
    assert.equal(fileB.sessionName, 'agent-b');
    assert.ok(fileA.current, 'session A has no current tab');
    assert.ok(fileB.current, 'session B has no current tab');
    assert.notEqual(fileA.current, fileB.current, 'sessions share the same current tab');

    const evalAenv = await run(['--cdp', wsUrl, 'eval', 'document.title'], cwd, {
      PLAYWRIGHT_CLI_SESSION: 'agent-a',
    });
    assert.equal(evalAenv.code, 0, `eval A env failed: ${evalAenv.stderr}`);
    assert.ok(evalAenv.stdout.includes('page-a'), `eval A env output: ${evalAenv.stdout}`);
  } finally {
    if (browser) {
      await browser.close().catch(() => {});
    }
    for (const p of cleanups) {
      await rm(p, { recursive: true, force: true }).catch(() => {});
    }
  }
});

test('sessions follow the invoking cwd: two cwds in parallel each keep their own tab', {
  timeout: 60_000,
}, async () => {
  const cwdA = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-cwd-a-'));
  const cwdB = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-cwd-b-'));
  const sessA = await sessionFile(cwdA);
  const sessB = await sessionFile(cwdB);
  const cleanups = [cwdA, cwdB, sessA, sessB];
  let browser;
  try {
    browser = await launchBrowser();
    const wsUrl = browser.wsUrl;
    const [openA, openB] = await Promise.all([
      run(['--cdp', wsUrl, 'open', 'about:blank'], cwdA),
      run(['--cdp', wsUrl, 'open', 'about:blank'], cwdB),
    ]);
    assert.equal(openA.code, 0, `open A failed: ${openA.stderr}`);
    assert.equal(openB.code, 0, `open B failed: ${openB.stderr}`);
    const [evalA, evalB] = await Promise.all([
      run(['--cdp', wsUrl, 'eval', 'document.title = "cwd-a"; document.title'], cwdA),
      run(['--cdp', wsUrl, 'eval', 'document.title = "cwd-b"; document.title'], cwdB),
    ]);
    assert.equal(evalA.code, 0, `eval A failed: ${evalA.stderr}`);
    assert.equal(evalB.code, 0, `eval B failed: ${evalB.stderr}`);
    const fileA = JSON.parse(await readFile(sessA, 'utf8'));
    const fileB = JSON.parse(await readFile(sessB, 'utf8'));
    assert.ok(fileA.current, 'cwd A has no current tab');
    assert.ok(fileB.current, 'cwd B has no current tab');
    assert.notEqual(fileA.current, fileB.current, 'two cwds share the same current tab');
    const [titleA, titleB] = await Promise.all([
      run(['--cdp', wsUrl, 'eval', 'document.title'], cwdA),
      run(['--cdp', wsUrl, 'eval', 'document.title'], cwdB),
    ]);
    assert.ok(titleA.stdout.includes('cwd-a'), `cwd A reads its own tab: ${titleA.stdout}`);
    assert.ok(titleB.stdout.includes('cwd-b'), `cwd B reads its own tab: ${titleB.stdout}`);
  } finally {
    if (browser) {
      await browser.close().catch(() => {});
    }
    for (const p of cleanups) {
      await rm(p, { recursive: true, force: true }).catch(() => {});
    }
  }
});

test('the saved session keeps no cookie, auth header, token query or request body', {
  timeout: 60_000,
}, async () => {
  const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-session-redact-'));
  const cleanups = [cwd];
  const secrets = [
    'CKSECRET1',
    'RSECRET2',
    'HSECRET3',
    'CSECRET4',
    'QSECRET5',
    'BSECRET6',
    'FSECRET7',
    'KSECRET88',
  ];
  const server = createServer((req, res) => {
    if (req.url.startsWith('/api')) {
      req.resume();
      res.writeHead(200, { 'content-type': 'text/plain', 'set-cookie': 'other=RSECRET2; Path=/' });
      res.end('done');
      return;
    }
    res.writeHead(200, { 'content-type': 'text/html', 'set-cookie': 'sid=CKSECRET1; Path=/' });
    res.end(
      '<!doctype html><title>redact</title><p>redact</p><script>console.log("auth Bearer " + "KSECRET" + "88"); console.log("visible note")</script>'
    );
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  let browser;
  try {
    browser = await launchBrowser();
    const wsUrl = browser.wsUrl;
    const file = await sessionFile(cwd);
    cleanups.push(file);

    const open = await run(
      ['--cdp', wsUrl, 'open', `${origin}/?access_token=QSECRET5#id_token=FSECRET7`],
      cwd
    );
    assert.equal(open.code, 0, `open failed: ${open.stderr}`);
    const fetched = await run(
      [
        '--cdp',
        wsUrl,
        'eval',
        "fetch('/api?token=QSECRET5&page=2', { method: 'POST', headers: { Authorization: 'Bearer HSECRET3', 'X-CSRF-Token': 'CSECRET4' }, body: 'BSECRET6' }).then((r) => r.text())",
      ],
      cwd
    );
    assert.equal(fetched.code, 0, `eval failed: ${fetched.stderr}`);
    assert.ok(fetched.stdout.includes('done'), `eval output: ${fetched.stdout}`);

    const text = await readFile(file, 'utf8');
    const saved = JSON.parse(text);
    const logged = Object.values(saved.requests ?? {}).flat();
    const api = logged.find((entry) => entry.url.includes('/api'));
    assert.ok(api, `the session has no /api request: ${text}`);
    assert.equal(api.url, `${origin}/api?token=[redacted]&page=2`);
    const header = (pairs, name) =>
      pairs.find(([key]) => key.toLowerCase() === name.toLowerCase())?.[1];
    assert.equal(header(api.requestHeaders, 'authorization'), '[redacted]');
    const notes = Object.values(saved.console ?? {}).flat();
    assert.ok(
      notes.some((entry) => entry.text === 'visible note'),
      `the harmless console entry was not saved: ${text}`
    );
    for (const secret of secrets) {
      assert.ok(!text.includes(secret), `${secret} is in the saved session: ${text}`);
    }
  } finally {
    if (browser) {
      await browser.close().catch(() => {});
    }
    await new Promise((resolve) => server.close(resolve));
    for (const p of cleanups) {
      await rm(p, { recursive: true, force: true }).catch(() => {});
    }
  }
});

async function withFixture(handler, body) {
  const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-session-plant-'));
  const cleanups = [cwd];
  const server = createServer(handler);
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  let browser;
  try {
    browser = await launchBrowser();
    const file = await sessionFile(cwd);
    cleanups.push(file);
    await body({ cwd, origin, file, wsUrl: browser.wsUrl });
  } finally {
    if (browser) {
      await browser.close().catch(() => {});
    }
    await new Promise((resolve) => server.close(resolve));
    for (const p of cleanups) {
      await rm(p, { recursive: true, force: true }).catch(() => {});
    }
  }
}

test('the saved session drops console entries that hold a bare token', {
  timeout: 60_000,
}, async () => {
  const tokens = {
    openai: ['sk-test', 'PLNT0123456789abcdefXYZ'],
    github: ['ghp_', 'PLNT0123456789abcdefghijklmnopqrstuv'],
    githubPat: ['github_pat_', '11PLNT0123456789_abcdefghijklmnopqrstuvwxyz'],
    aws: ['AKIA', 'PLNT0123456789AB'],
    slack: ['xoxb-', '1234567890-PLNTabcdef'],
    gitlab: ['glpat-', 'PLNT0123456789abcdefgh'],
    entropy: ['Zk9xL3Rq2mWv8sPb7NcYd4hGe1', 'RaKu6TfQi0Xo5Jz9PLNT'],
    stripe: ['sk_live_', 'PLNT0123456789abcdefgh'],
    google: ['AIza', 'SyPLNT0123456789abcdefghijklmnopqrs'],
    npm: ['npm_', 'PLNT0123456789abcdefghijklmnopqrstu'],
    huggingface: ['hf_', 'PLNTabcdefghijklmnopqrstuvwxyzABCDEFGH'],
    pypi: ['pypi-', 'AgEIcHlwaS5vcmcPLNT0123456789abcdefghij'],
    lowercase: ['7f3k9q2m8x1v4b6n0c5z', '7l2j9h4g1d8s6a3pplnt'],
  };
  const logs = Object.values(tokens)
    .map(
      ([head, tail]) => `console.log("value " + ${JSON.stringify(head)} + ${JSON.stringify(tail)});`
    )
    .join('');
  const handler = (_req, res) => {
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end(
      `<!doctype html><title>plant</title><script>${logs}console.log("a plain sentence stays")</script>`
    );
  };
  await withFixture(handler, async ({ cwd, origin, file, wsUrl }) => {
    const open = await run(['--cdp', wsUrl, 'open', `${origin}/`], cwd);
    assert.equal(open.code, 0, `open failed: ${open.stderr}`);
    const text = await readFile(file, 'utf8');
    const notes = Object.values(JSON.parse(text).console ?? {}).flat();
    assert.ok(
      notes.some((entry) => entry.text === 'a plain sentence stays'),
      `the plain console entry was not saved: ${text}`
    );
    for (const [head, tail] of Object.values(tokens)) {
      assert.ok(!text.includes(tail), `${head}${tail} is in the saved session: ${text}`);
    }
  });
});

test('the saved session redacts the query of a relative Location header', {
  timeout: 60_000,
}, async () => {
  const handler = (req, res) => {
    if (req.url.startsWith('/go')) {
      res.writeHead(302, { location: '/next?token=PLNTLOC330&code=PLNTCODE330&page=2' });
      res.end();
      return;
    }
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end('<!doctype html><title>next</title><p>next</p>');
  };
  await withFixture(handler, async ({ cwd, origin, file, wsUrl }) => {
    const open = await run(['--cdp', wsUrl, 'open', `${origin}/go`], cwd);
    assert.equal(open.code, 0, `open failed: ${open.stderr}`);
    const text = await readFile(file, 'utf8');
    const logged = Object.values(JSON.parse(text).requests ?? {}).flat();
    const location = logged
      .flatMap((entry) => entry.responseHeaders ?? [])
      .find(([name]) => name.toLowerCase() === 'location');
    assert.ok(location, `the redirect has no Location header: ${text}`);
    for (const secret of ['PLNTLOC330', 'PLNTCODE330']) {
      assert.ok(!text.includes(secret), `${secret} is in the saved session: ${text}`);
    }
    assert.equal(location[1], '/next?token=[redacted]&code=[redacted]&page=2');
  });
});

test('the saved session keeps no response body and response-body says it was not saved', {
  timeout: 60_000,
}, async () => {
  const handler = (_req, res) => {
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end(
      '<!doctype html><title>body</title><script>window.apiKey = "PLNT-PAGE-TOKEN-331"</script><p>body</p>'
    );
  };
  await withFixture(handler, async ({ cwd, origin, file, wsUrl }) => {
    const open = await run(['--cdp', wsUrl, 'open', `${origin}/page`], cwd);
    assert.equal(open.code, 0, `open failed: ${open.stderr}`);
    const text = await readFile(file, 'utf8');
    assert.ok(
      !text.includes('PLNT-PAGE-TOKEN-331'),
      `the page body is in the saved session: ${text}`
    );
    const logged = Object.values(JSON.parse(text).requests ?? {}).flat();
    const index = logged.findIndex((entry) => entry.url === `${origin}/page`) + 1;
    assert.ok(index > 0, `the session has no page request: ${text}`);
    assert.ok(
      logged.every((entry) => entry.responseBody == null),
      `a body was saved: ${text}`
    );
    const details = await run(['--cdp', wsUrl, 'request', String(index)], cwd);
    assert.equal(details.code, 0, `request failed: ${details.stderr}`);
    assert.ok(
      details.stdout.includes('The response body is not kept in the saved session.'),
      details.stdout
    );
    const body = await run(['--cdp', wsUrl, 'response-body', String(index)], cwd);
    assert.notEqual(body.code, 0, `response-body succeeded: ${body.stdout}`);
    assert.equal(
      body.stderr,
      `playwright-cli response-body: request #${index}: The response body is not kept in the saved session.\n`
    );
    assert.ok(!body.stdout.includes('PLNT-PAGE-TOKEN-331'), body.stdout);
  });
});
