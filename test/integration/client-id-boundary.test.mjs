import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { Script } from 'node:vm';

const source = await readFile(new URL('../../src/cdp-client.js', import.meta.url), 'utf8');
const maximum = 2 ** 30 - 1;

class Socket {
  static last;
  listeners = new Map();
  sent = [];

  constructor() {
    Socket.last = this;
    queueMicrotask(() => this.emit('open', {}));
  }

  addEventListener(name, listener) {
    this.listeners.set(name, listener);
  }

  emit(name, value) {
    this.listeners.get(name)?.(value);
  }

  send(text) {
    this.sent.push(JSON.parse(text));
  }

  close() {}
}

function clientAt(counter) {
  assert.ok(source.includes('#nextId = 0;'));
  const seeded = source
    .replace('#nextId = 0;', `#nextId = ${counter};`)
    .replace('export class CDPClient', 'globalThis.CDPClient = class CDPClient');
  const context = { setTimeout, clearTimeout, queueMicrotask };
  new Script(seeded, {
    filename: new URL('../../src/cdp-client.js', import.meta.url).pathname,
  }).runInNewContext(context);
  return new context.CDPClient(Socket);
}

test('client message ids stop before the range boundary without disturbing the final reply', async () => {
  const client = clientAt(maximum - 1);
  await client.open('ws://fake.invalid');
  const socket = Socket.last;
  try {
    const pending = client.send('Browser.getVersion');
    assert.equal(socket.sent[0].id, maximum);
    await assert.rejects(
      client.send('Browser.getVersion'),
      /CDP client message ID range exhausted/
    );
    await assert.rejects(
      client.send('Browser.getVersion'),
      /CDP client message ID range exhausted/
    );
    assert.equal(socket.sent.length, 1);
    socket.emit('message', {
      data: JSON.stringify({ id: maximum, result: { product: 'last permitted reply' } }),
    });
    assert.equal((await pending).product, 'last permitted reply');
  } finally {
    client.close();
  }
});

test('an already exhausted client counter never emits a reserved or wrapped id', async () => {
  const client = clientAt(Number.MAX_SAFE_INTEGER);
  await client.open('ws://fake.invalid');
  try {
    await assert.rejects(
      client.send('Browser.getVersion'),
      /CDP client message ID range exhausted/
    );
    assert.equal(Socket.last.sent.length, 0);
  } finally {
    client.close();
  }
});
