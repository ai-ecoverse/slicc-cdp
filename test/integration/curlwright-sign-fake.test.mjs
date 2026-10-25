import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createPublicKey, verify } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { createServer } from 'node:http';
import path from 'node:path';
import { before, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { fakeCdp } from './fake-cdp.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const vectors = JSON.parse(
  readFileSync(path.join(root, 'test/integration/fixtures/web-bot-auth-vectors.json'), 'utf8')
).vectors;
const key = vectors[0].key;
const rsa = {
  kty: 'RSA',
  e: 'AQAB',
  n: 'r4tmm3r20Wd',
  d: 'lAfIqfpCYomVShfAKnwf2lD9I0wKjkHsCtZCif4kAlw',
};
const signed = ['signature', 'signature-input', 'signature-agent'];

before(() => {
  execFileSync('cargo', ['build', '--locked', '-p', 'curlwright'], { stdio: 'inherit' });
  execFileSync(
    'cargo',
    ['build', '--locked', '--target', 'wasm32-wasip1', '-p', 'slicc-cdp', '-p', 'curlwright'],
    {
      stdio: 'inherit',
    }
  );
});

const runners = {
  native: (url) => nativeRunner(null, path.join(root, 'target/debug/curlwright'), url),
  kernel: (url) =>
    kernelRunner(
      path.join(root, 'target/wasm32-wasip1/debug/playwright-cli.wasm'),
      path.join(root, 'target/wasm32-wasip1/debug/curlwright.wasm'),
      url
    ),
};

function listen(handler) {
  const server = createServer(handler);
  return new Promise((resolve) =>
    server.listen(0, '127.0.0.1', () =>
      resolve({ server, base: `http://127.0.0.1:${server.address().port}` })
    )
  );
}

function browser(base) {
  let requestId = 0;
  return {
    intercept(message, send) {
      if (
        message.method !== 'Runtime.evaluate' ||
        !message.params.expression.includes('const __init =')
      )
        return false;
      const browserFetch = async (url, init) => {
        const source = new URL(url).href;
        const id = `R${++requestId}`;
        send({
          method: 'Network.requestWillBeSent',
          sessionId: message.sessionId,
          params: { requestId: id, request: { url: source, method: init.method, headers: {} } },
        });
        const response = await fetch(source, { ...init, redirect: 'manual' });
        const location = response.headers.get('location');
        const metadata = {
          url: source,
          status: response.status,
          headers: Object.fromEntries(response.headers),
        };
        if (location && [301, 302, 303, 307, 308].includes(response.status)) {
          send({
            method: 'Network.requestWillBeSent',
            sessionId: message.sessionId,
            params: {
              requestId: id,
              request: { url: new URL(location, source).href, method: init.method, headers: {} },
              redirectResponse: metadata,
            },
          });
          return { type: 'opaqueredirect' };
        }
        send({
          method: 'Network.responseReceived',
          sessionId: message.sessionId,
          params: { requestId: id, response: metadata },
        });
        return response;
      };
      const evaluate = new Function('fetch', 'location', `return ${message.params.expression}`);
      Promise.resolve()
        .then(() => evaluate(browserFetch, { href: `${base}/page` }))
        .then(
          (value) => send({ id: message.id, result: { result: { value } } }),
          (error) => send({ id: message.id, result: { exceptionDetails: { text: String(error) } } })
        );
      return true;
    },
  };
}

async function scenario(kind, body) {
  const seen = [];
  const record = (req) =>
    seen.push({
      authority: req.headers.host,
      path: req.url,
      headers: Object.fromEntries(signed.map((name) => [name, req.headers[name]])),
    });
  const landing = await listen((req, res) => {
    record(req);
    res.end('landed');
  });
  const start = await listen((req, res) => {
    record(req);
    res.writeHead(302, { Location: `${landing.base}/landing` });
    res.end();
  });
  const backend = await fakeCdp(browser(start.base));
  backend.targets.push({ targetId: 'TAB', url: `${start.base}/page`, loaderId: 'L' });
  let runner;
  try {
    runner = await runners[kind](backend.url);
    const cwd = await runner.fresh();
    await runner.write(cwd, 'key.jwk', JSON.stringify(key));
    await runner.write(cwd, 'rsa.jwk', JSON.stringify(rsa));
    await body({
      seen,
      start: start.base,
      landing: landing.base,
      curl: (args) => runner.curl(['--tab', 'TAB', ...args], cwd),
    });
  } finally {
    await Promise.resolve(runner?.close()).catch(() => {});
    await backend.close();
    for (const { server } of [start, landing]) {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    }
  }
}

function verifyHop(hop, agent) {
  const input = hop.headers['signature-input'];
  const params = input.replace(/^sig1=/, '');
  const covered = agent ? '("@authority" "signature-agent";key="sig1")' : '("@authority")';
  assert.ok(params.startsWith(`${covered};created=`), input);
  const created = Number(/;created=(\d+)/.exec(params)[1]);
  const expires = Number(/;expires=(\d+)/.exec(params)[1]);
  assert.equal(expires - created, 60);
  assert.ok(Math.abs(created - Date.now() / 1000) < 60, input);
  assert.match(params, /;keyid="poqkLGiymh_W0uP6PZFw-dvez3QJT5SolqXBCW38r0U";alg="ed25519";/);
  assert.match(params, /;nonce="[A-Za-z0-9+/]{86}==";tag="web-bot-auth"$/);
  const lines = [`"@authority": ${hop.authority}`];
  if (agent) {
    assert.equal(hop.headers['signature-agent'], `sig1="${agent}"`);
    lines.push(`"signature-agent";key="sig1": "${agent}"`);
  } else {
    assert.equal(hop.headers['signature-agent'], undefined);
  }
  lines.push(`"@signature-params": ${params}`);
  const signature = /^sig1=:([A-Za-z0-9+/=]+):$/.exec(hop.headers.signature)[1];
  const publicKey = createPublicKey({
    key: { kty: key.kty, crv: key.crv, x: key.x },
    format: 'jwk',
  });
  assert.ok(
    verify(null, Buffer.from(lines.join('\n')), publicKey, Buffer.from(signature, 'base64')),
    `${hop.authority} signature does not verify`
  );
}

for (const kind of Object.keys(runners)) {
  test(`${kind} curlwright sends no Web Bot Auth headers without --sign-key`, async () => {
    await scenario(kind, async ({ seen, start, curl }) => {
      const result = await curl([`${start}/start`]);
      assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
      assert.equal(result.stdout, 'landed');
      assert.equal(seen.length, 2);
      for (const hop of seen)
        for (const name of signed) assert.equal(hop.headers[name], undefined, name);
    });
  });

  test(`${kind} curlwright signs every redirect hop for its own authority`, async () => {
    await scenario(kind, async ({ seen, start, landing, curl }) => {
      const agent = 'https://signature-agent.test';
      const result = await curl([
        '--sign-key',
        'key.jwk',
        '--signature-agent',
        agent,
        `${start}/start`,
      ]);
      assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
      assert.equal(result.stdout, 'landed');
      assert.deepEqual(
        seen.map((hop) => `${hop.authority}${hop.path}`),
        [`${new URL(start).host}/start`, `${new URL(landing).host}/landing`]
      );
      for (const hop of seen) verifyHop(hop, agent);
      assert.notEqual(seen[0].headers.signature, seen[1].headers.signature);
      seen.length = 0;
      const bare = await curl(['--sign-key', 'key.jwk', `${start}/start`]);
      assert.equal(bare.code, 0, `${bare.stdout}\n${bare.stderr}`);
      assert.equal(seen.length, 2);
      for (const hop of seen) verifyHop(hop, null);
    });
  });

  test(`${kind} curlwright refuses a key that is not Ed25519 without printing it`, async () => {
    await scenario(kind, async ({ seen, start, curl }) => {
      const result = await curl(['--sign-key', 'rsa.jwk', `${start}/start`]);
      assert.equal(result.code, 2, result.stderr);
      assert.match(result.stderr, /--sign-key rsa\.jwk is not an Ed25519 key/);
      assert.equal(result.stderr.includes(rsa.d), false);
      assert.equal(seen.length, 0);
    });
  });
}
