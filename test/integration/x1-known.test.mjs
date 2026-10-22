import assert from 'node:assert/strict';
import test from 'node:test';
import { expectedFailureDispositions, verdictProblems } from './guard-set.mjs';
import {
  isExpectedX1Failure150,
  protectedX1Exposure,
  recordX1Client,
  x1Privacy,
} from './x1-known.mjs';

const ordinary = 'http://plain.guard.test:18100';
const protectedOrigin = 'http://seven.sliccy.ai:18100';
function evidence() {
  const actors = [ordinary, protectedOrigin].map((destination, index) => {
    const sessionId = `S${index}`;
    const loaderId = `N${index}`;
    const url = `${ordinary}/page?k=href&tag=public-${index}&to=${destination}/canary/public-${index}`;
    return {
      closed: true,
      calls: [
        {
          method: 'Page.addScriptToEvaluateOnNewDocument',
          error: 'slicc: refused: persistent page capabilities are not supported by slicc-cdp\n',
        },
        {
          method: 'Network.setExtraHTTPHeaders',
          error: 'slicc: refused: persistent page capabilities are not supported by slicc-cdp\n',
        },
        { method: 'Page.navigate', params: { url }, sessionId, result: { loaderId } },
      ],
      messages: [
        JSON.stringify({
          method: 'Network.requestWillBeSent',
          sessionId,
          params: { requestId: loaderId, request: { url } },
        }),
        JSON.stringify({
          method: 'Fetch.requestPaused',
          sessionId,
          params: { requestId: `F${index}`, networkId: loaderId, request: { url } },
        }),
      ],
    };
  });
  const observed = {
    instrumented: false,
    headed: false,
    intercepted: true,
    served: true,
    document: true,
    forged: false,
    scripted: false,
    probed: false,
    paused: false,
  };
  const row = {
    id: 'X1.after-exit',
    target: 'seven',
    variant: 'prompt',
    control: 'fail',
    guarded: 'fail',
    live: 'pass',
    observations: { control: { ...observed }, guarded: { ...observed } },
    errors: [
      `settle: no beacon from ${ordinary} within 1500ms`,
      `settle: no beacon from ${ordinary} within 1500ms`,
    ],
  };
  return { actors, row };
}

test('#150 preserves raw X1 failures and binds only exact authored request URL echoes', () => {
  const { actors, row } = evidence();
  const original = structuredClone(row);
  row.x1Privacy = x1Privacy(actors, { ordinary, protectedOrigin });
  assert.equal(isExpectedX1Failure150(row), true);
  assert.deepEqual(verdictProblems([row]), []);
  assert.equal(row.x1Privacy.echoes.length, 4);
  assert(
    row.x1Privacy.echoes.every((echo) => echo.byteExact && echo.outgoingURL === echo.incomingURL)
  );
  assert.deepEqual(row.observations, original.observations);
  assert.equal(row.control, 'fail');
  assert.equal(row.guarded, 'fail');
  const [disposition] = expectedFailureDispositions([row]);
  assert.equal(disposition.reason, 'slicc-browsers#150');
  assert.match(disposition.deletionHook, /fixed X1 vector/);
  for (const target of ['seven', 'vault'])
    for (const variant of ['prompt', 'lagging'])
      assert.equal(isExpectedX1Failure150({ ...row, target, variant }), true);
});

test('#150 never excludes a private marker, a differing request or a URL in any other field', () => {
  const mutate = [
    (message) => {
      message.params.requestId = 'OTHER';
      delete message.params.networkId;
    },
    (message) => {
      message.sessionId = 'OTHER';
    },
    (message) => {
      message.params.request.url += '&new=1';
    },
    (message) => {
      message.params.headers = { Location: `${protectedOrigin}/canary/private` };
    },
    (message) => {
      message.params.frame = { url: `${protectedOrigin}/canary/private` };
    },
    (message) => {
      message.params.title = `${protectedOrigin}/canary/private`;
    },
    (message) => {
      message.params.body = `canary-marker ${new URL(protectedOrigin).host}`;
    },
    (message) => {
      message.params.body = Buffer.from(`${protectedOrigin}/canary/private`).toString('base64');
    },
    (message) => {
      message.params.other = JSON.stringify({ url: `${protectedOrigin}/canary/private` });
    },
    (message) => {
      message.method = 'Network.responseReceived';
    },
  ];
  for (const change of mutate) {
    const { actors, row } = evidence();
    const message = JSON.parse(actors[1].messages[1]);
    change(message);
    actors[1].messages[1] = JSON.stringify(message);
    row.x1Privacy = x1Privacy(actors, { ordinary, protectedOrigin });
    assert.equal(isExpectedX1Failure150(row), false);
    assert.ok(verdictProblems([row]).length > 0);
    assert.ok(verdictProblems([{ ...row, control: 'pass', guarded: 'pass' }]).length > 0);
  }
  const { actors, row } = evidence();
  const url =
    actors[1].calls[2].params.url +
    `&marker=${encodeURIComponent(`canary-marker ${new URL(protectedOrigin).host}`)}`;
  actors[1].calls[2].params.url = url;
  const event = JSON.parse(actors[1].messages[1]);
  event.params.request.url = url;
  actors[1].messages[1] = JSON.stringify(event);
  row.x1Privacy = x1Privacy(actors, { ordinary, protectedOrigin });
  assert.equal(row.x1Privacy.markerExposed, true);
  assert.equal(isExpectedX1Failure150(row), false);
});

test('#150 rejects screenshots, missing audit, other failures and incorrect legs', () => {
  const actorsChange = [
    (actors) => {
      actors[1].closed = false;
    },
    (actors) => {
      actors[1].messages = [];
    },
    (actors) => {
      actors[1].messages.push('invalid JSON');
    },
    (actors) => {
      actors[1].messages.push('null');
    },
    (actors) => {
      actors[1].calls[2].result.loaderId = 'OTHER';
    },
    (actors) => {
      actors[1].calls[0].error = 'unknown setup failure';
    },
    (actors) => {
      actors[1].calls.push({
        method: 'Page.captureScreenshot',
        result: { data: 'PRIVATE_SCREENSHOT' },
      });
    },
  ];
  for (const change of actorsChange) {
    const { actors, row } = evidence();
    change(actors);
    row.x1Privacy = x1Privacy(actors, { ordinary, protectedOrigin });
    assert.equal(isExpectedX1Failure150(row), false);
  }
  for (const change of [
    (r) => {
      r.id = 'OTHER';
    },
    (r) => {
      r.target = 'ordinary';
    },
    (r) => {
      r.variant = 'other';
    },
    (r) => {
      r.live = 'fail';
    },
    (r) => {
      r.control = 'pass';
    },
    (r) => {
      r.guarded = 'incomplete';
    },
    (r) => {
      delete r.x1Privacy;
    },
    (r) => {
      r.errors.push('other error');
    },
    (r) => {
      r.observations.guarded.scripted = true;
    },
    (r) => {
      r.observations.control.instrumented = true;
      r.observations.control.headed = true;
    },
  ]) {
    const { actors, row } = evidence();
    row.x1Privacy = x1Privacy(actors, { ordinary, protectedOrigin });
    change(row);
    assert.equal(isExpectedX1Failure150(row), false);
  }
});

test('private URL audit decodes nested carriers and case variants', () => {
  for (const encode of [
    (s) => s,
    encodeURIComponent,
    (s) => Buffer.from(s).toString('base64'),
    (s) => [...s].map((c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, '0')}`).join(''),
  ])
    assert.equal(protectedX1Exposure(encode('HTTPS://SEVEN.SLICCY.AI:18100/canary/secret')), true);
  assert.equal(protectedX1Exposure({ value: 'ordinary public bytes' }), false);
});

test('X1 recorder captures actual outgoing results, refusals and completed close', async () => {
  const client = {
    messages: ['{}'],
    async send(method) {
      if (method === 'denied') throw new Error('actual refusal');
      return { loaderId: 'actual-loader' };
    },
    async close() {},
  };
  const actor = recordX1Client(client);
  await client.send('Page.navigate', { url: `${ordinary}/page` }, 'S');
  await assert.rejects(client.send('denied'), /actual refusal/);
  assert.equal(actor.calls[0].result.loaderId, 'actual-loader');
  assert.equal(actor.calls[1].error, 'actual refusal');
  assert.equal(actor.closed, false);
  await client.close();
  assert.equal(actor.closed, true);
});
