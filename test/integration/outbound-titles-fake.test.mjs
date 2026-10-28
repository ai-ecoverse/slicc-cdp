import assert from 'node:assert/strict';
import { performance } from 'node:perf_hooks';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';
import './watchdog.mjs';

const ordinary = 'https://ordinary.example/blank';
const extra = 'http://192.0.2.17:9443';
const suffix = '/doc/path-secret-fixture?query-secret=fixture#fragment-secret-fixture';
const full = (text) =>
  [...new TextEncoder().encode(text)]
    .map((byte) => `%${byte.toString(16).toUpperCase().padStart(2, '0')}`)
    .join('');
const layer = (text) => text.replaceAll('%', '%25');
const origins = [
  'https://vault.sliccy.ai',
  'https://seven.sliccy.ai',
  'https://tray-fixture.sliccy.ai',
  'https://controller-fixture.sliccy.ai',
  extra,
  'HTTPS://VaUlT.SlIcCy.Ai',
];
const rows = origins.flatMap((origin) => [
  {
    name: `${origin} layered`,
    title: layer(full(origin + suffix)),
    expected: origin.toLowerCase(),
  },
  {
    name: `${origin} unschemed`,
    title: origin.split('://')[1] + suffix,
    expected: origin.split('://')[1],
  },
]);
let overflow = full(origins[0] + suffix);
for (let index = 1; index < 6; index++) overflow = layer(overflow);
rows.push({ name: 'decode limit', title: overflow, expected: '' });
rows.push({
  name: 'literal origin decode limit',
  title: origins[0] + '/' + overflow,
  expected: '',
});
rows.push({
  name: 'invalid encoded bytes',
  title: '%FF' + layer(full(origins[0] + suffix)),
  expected: '%FF' + origins[0],
});
rows.push({
  name: 'unicode tray host',
  title: 'é.tray-fixture.sliccy.ai' + suffix,
  expected: 'é.tray-fixture.sliccy.ai',
});
rows.push({
  name: 'nested query separators',
  title:
    'https://ordinary.example/?next=' +
    layer(full(origins[0] + suffix + '&second=fixture')) +
    '&control=ordinary',
  expected: 'https://ordinary.example/?next=' + origins[0] + '&control=ordinary',
});
rows.push({
  name: 'literal nested query',
  title: 'https://ordinary.example/?next=' + origins[0] + suffix + '&second=fixture',
  expected: 'https://ordinary.example/?next=' + origins[0],
});
for (const title of [
  ordinary + '?host=seven.sliccy.ai&path=visible',
  ordinary + '?host=vault.sliccy.ai&path=visible',
  '%cText',
  '%cDownload',
  '100%ff',
  'caf%E9',
  '100%ff done',
  'https://ordinary.example/caf%E9',
])
  rows.push({ name: 'ordinary text ' + title, title, expected: title });
rows.push({
  name: 'non-hex escape before unicode separator',
  title: 'https://ordinary.example/?next=https%3A%2F%2Fvault.sliccy.ai%2Fs%+F%+F&éééé',
  expected: 'https://ordinary.example/?next=' + origins[0] + '&éééé',
});
rows.push({
  name: 'ordinary formatting beside a protected title',
  title: 'before%cDownload ' + layer(full(origins[0] + suffix)) + ' after',
  expected: 'before%cDownload ' + origins[0] + ' after',
});
rows.push({
  name: 'encoded unicode tray host',
  title: layer(full('é.tray-fixture.sliccy.ai' + suffix)),
  expected: 'é.tray-fixture.sliccy.ai',
});
const outer = ordinary + '?keep=visible%20bytes%2B%41&to=';
const control = '&tail=visible%252Fbytes#ordinary-fragment';
for (const origin of origins) {
  const host = origin.split('://')[1];
  for (const reference of [
    host + suffix,
    host + '%2Fdoc%2Fpath-secret-fixture%3Fquery-secret%3Dfixture%23fragment-secret-fixture',
    layer(full(host + suffix)),
  ])
    rows.push({
      name: `nested unschemed ${origin} ${reference.startsWith('%') ? 'layered' : reference.includes('%') ? 'encoded path' : 'literal'}`,
      title: outer + reference + control,
      expected: outer + host + control,
    });
}
rows.push({
  name: 'nested unschemed outer host',
  title: 'ordinary.example/page?to=vault.sliccy.ai/doc/path-secret-fixture&tail=visible',
  expected: 'ordinary.example/page?to=vault.sliccy.ai&tail=visible',
});
rows.push({
  name: 'nested unschemed unicode',
  title: outer + layer(full('é.tray.sliccy.ai' + suffix)) + control,
  expected: outer + 'é.tray.sliccy.ai' + control,
});
rows.push({
  name: 'nested unschemed repeated references',
  title: outer + 'vault.sliccy.ai/private&second=seven.sliccy.ai/private' + control,
  expected: outer + 'vault.sliccy.ai&second=seven.sliccy.ai' + control,
});
rows.push({
  name: 'schemed nested ordinary bytes',
  title: outer + layer(full(origins[0] + suffix)) + control,
  expected: outer + origins[0] + control,
});
const repeated =
  ordinary + '?' + 'kept=vault.sliccy.ai&kept=seven.sliccy.ai&'.repeat(10000) + 'to=';
rows.push({
  name: 'nested unschemed long crafted title',
  title: repeated + 'vault.sliccy.ai/doc/path-secret-fixture' + control,
  expected: repeated + 'vault.sliccy.ai' + control,
  bounded: true,
});
for (const prefix of [
  'https://ordinary.example/?u=',
  'ordinary.example/?u=',
  'https://ordinary.example/a?b#',
])
  for (const host of ['vault.sliccy.ai', 'seven.sliccy.ai'])
    for (const userinfo of ['a&b', 'a%26b&c', 'a&b%26c', 'a&b@', 'a&b@@'])
      rows.push({
        name: `userinfo authority ${prefix} ${userinfo} ${host}`,
        title: prefix + 'https://' + userinfo + '@' + host + '/doc/SECRETX',
        expected: prefix + 'https://' + host,
      });
for (const prefix of ['x?to=', 'ordinary.example/?to=', 'https://ordinary.example/?to='])
  for (const separator of [';', ',', '!', '$', '%27', '('])
    rows.push({
      name: `bare host punctuation ${prefix} ${separator}`,
      title: prefix + 'vault.sliccy.ai' + separator + 'SECRETX&tail=visible%252F',
      expected: prefix + 'vault.sliccy.ai&tail=visible%252F',
    });
rows.push({
  name: 'bare host literal quoted boundary',
  title: "x?to=vault.sliccy.ai;SECRETX'ordinary",
  expected: "x?to=vault.sliccy.ai'ordinary",
});
const authorityBudget =
  'https://ordinary.example/?u=' + 'https:a&'.repeat(12000) + '@ordinary.example';
rows.push({
  name: 'overlapping authorities long crafted title',
  title: authorityBudget,
  expected: '',
  bounded: true,
});
const prefix = 'https://ordinary.example/'.repeat(12000);
rows.push({
  name: 'long crafted title',
  title: prefix + layer(full(origins[0] + suffix)),
  expected: prefix + origins[0],
  bounded: true,
});
rows.push({
  name: 'ordinary encoded title',
  title: layer(full(ordinary)),
  expected: layer(full(ordinary)),
});

for (const transport of ['native', 'wasi']) {
  test(`${transport}: outbound titles retain origins across events and replies`, async (t) => {
    let row = { title: 'ordinary' };
    let session;
    let burst = false;
    let refusing = false;
    const queryUrl = ordinary + '?host=seven.sliccy.ai&path=visible';
    const info = (target) => ({ ...target, type: 'page', title: row.title });
    const frame = () => ({
      id: 'ORDINARY',
      loaderId: 'L',
      url: ordinary,
      securityOrigin: new URL(ordinary).origin,
      name: row.title,
    });
    const fake = await fakeCdp({
      intercept(message, send) {
        const reply = (result) => send({ id: message.id, sessionId: message.sessionId, result });
        if (message.method === 'Browser.getVersion' && refusing) {
          refusing = false;
          send({ id: message.id, error: { code: -32000, message: row.title } });
          return true;
        }
        if (message.method === 'Target.getTargets') {
          reply({ targetInfos: fake.targets.map(info) });
          return true;
        }
        if (message.method === 'Target.getTargetInfo') {
          const target = fake.targets.find(
            (item) => item.targetId === (message.params.targetId ?? fake.calls.at(-1).target)
          );
          if (!target) return false;
          reply({ targetInfo: info(target) });
          return true;
        }
        if (['Page.getFrameTree', 'Page.getResourceTree'].includes(message.method)) {
          reply({ frameTree: { frame: frame(), childFrames: [] } });
          return true;
        }
        if (message.method === 'Runtime.evaluate') {
          reply({
            result: { type: 'undefined', description: row.title },
            exceptionDetails: { text: row.title, exception: { description: row.title } },
          });
          return true;
        }
        if (message.method === 'Browser.getVersion' && burst) {
          burst = false;
          for (const method of [
            'Target.targetCreated',
            'Target.targetInfoChanged',
            'Target.attachedToTarget',
          ])
            send({
              method,
              params: {
                targetInfo: info(fake.targets[0]),
                sessionId: session,
                waitingForDebugger: false,
              },
            });
          for (const [method, params] of [
            [
              'Network.requestWillBeSent',
              {
                requestId: 'QUERY',
                loaderId: 'L',
                frameId: 'ORDINARY',
                type: 'XHR',
                request: { url: queryUrl, method: 'GET', headers: {} },
              },
            ],
            ['Page.frameNavigated', { frame: frame() }],
            [
              'Runtime.consoleAPICalled',
              {
                type: 'log',
                executionContextId: 1,
                args: [{ type: 'string', value: row.title, description: row.title }],
                stackTrace: { description: row.title },
              },
            ],
            [
              'Runtime.exceptionThrown',
              {
                exceptionDetails: {
                  text: row.title,
                  exception: { description: row.title },
                  stackTrace: { description: row.title },
                },
              },
            ],
            [
              'Log.entryAdded',
              { entry: { source: 'javascript', level: 'error', text: row.title, url: ordinary } },
            ],
          ])
            send({ method, params, sessionId: session });
        }
        return false;
      },
    });
    fake.targets.push(
      { targetId: 'ORDINARY', url: ordinary, loaderId: 'L' },
      { targetId: 'PROTECTED', url: origins[0] + suffix }
    );
    const client = await guarded(fake.url, extra, { variant: 'prompt', detachDelay: 0, transport });
    try {
      ({ sessionId: session } = await client.send('Target.attachToTarget', {
        targetId: 'ORDINARY',
        flatten: true,
      }));
      await client.send('Runtime.enable', {}, session);
      for (const next of rows) {
        await t.test(next.name, async () => {
          row = next;
          const started = performance.now();
          const listing = await client.send('Target.getTargets');
          assert.equal(listing.targetInfos[0].title, row.expected);
          assert.equal(listing.targetInfos[0].url, ordinary);
          if (row.bounded) {
            assert.ok(performance.now() - started < 5000, 'bounded title scan');
            return;
          }
          const target = await client.send('Target.getTargetInfo', { targetId: 'ORDINARY' });
          assert.equal(target.targetInfo.title, row.expected);
          assert.equal(target.targetInfo.url, ordinary);
          const tree = await client.send('Page.getFrameTree', {}, session);
          assert.equal(tree.frameTree.frame.name, row.expected);
          assert.equal(tree.frameTree.frame.url, ordinary);
          const resources = await client.send('Page.getResourceTree', {}, session);
          assert.equal(resources.frameTree.frame.name, row.expected);
          assert.equal(resources.frameTree.frame.url, ordinary);
          const evaluated = await client.send('Runtime.evaluate', { expression: '1' }, session);
          assert.equal(evaluated.result.description, row.expected);
          assert.equal(evaluated.exceptionDetails.text, row.expected);
          assert.equal(evaluated.exceptionDetails.exception.description, row.expected);
          const from = client.events.length;
          refusing = true;
          await assert.rejects(
            client.send('Browser.getVersion'),
            (error) => error.message === `Browser.getVersion: ${row.expected}\n`
          );
          burst = true;
          await client.send('Browser.getVersion');
          await client.send('Browser.getVersion');
          const events = client.events.slice(from);
          const request = events.find((event) => event.method === 'Network.requestWillBeSent');
          assert.ok(request);
          assert.equal(request.params.request.url, queryUrl);
          for (const method of [
            'Target.targetCreated',
            'Target.targetInfoChanged',
            'Target.attachedToTarget',
          ]) {
            const event = events.find((item) => item.method === method);
            assert.ok(event, method);
            assert.equal(event.params.targetInfo.title, row.expected, method);
            assert.equal(event.params.targetInfo.url, ordinary, method);
          }
          for (const [method, fields] of [
            ['Page.frameNavigated', [['frame', 'name']]],
            [
              'Runtime.consoleAPICalled',
              [
                ['args', 0, 'description'],
                ['args', 0, 'value'],
                ['stackTrace', 'description'],
              ],
            ],
            [
              'Runtime.exceptionThrown',
              [
                ['exceptionDetails', 'text'],
                ['exceptionDetails', 'exception', 'description'],
                ['exceptionDetails', 'stackTrace', 'description'],
              ],
            ],
            ['Log.entryAdded', [['entry', 'text']]],
          ]) {
            const event = events.find((item) => item.method === method);
            assert.ok(event, method);
            for (const field of fields)
              assert.equal(
                field.reduce((value, key) => value[key], event.params),
                row.expected,
                method
              );
          }
        });
      }
      const from = fake.calls.length;
      await assert.rejects(
        client.send('Target.attachToTarget', { targetId: 'PROTECTED', flatten: true }),
        /slicc: refused:/
      );
      await assert.rejects(
        client.send('Page.navigate', { url: origins[0] + suffix }, session),
        /slicc: refused:/
      );
      await assert.rejects(
        client.send('Runtime.addBinding', { name: 'fixture' }, session),
        /slicc: refused:/
      );
      assert.equal(
        fake.calls
          .slice(from)
          .some((call) => ['Page.navigate', 'Runtime.addBinding'].includes(call.method)),
        false
      );
    } finally {
      await client.close();
      await fake.close();
    }
  });
}
