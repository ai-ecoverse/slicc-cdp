import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import net from 'node:net';
import test from 'node:test';
import { constants, deflateRawSync } from 'node:zlib';
import { cdpProxy } from './cdp-proxy.mjs';
import { InboundGate } from './inbound-gate.mjs';

function frame(message, compressed) {
  const plain = Buffer.from(JSON.stringify(message));
  const payload = compressed
    ? deflateRawSync(plain, { finishFlush: constants.Z_SYNC_FLUSH }).subarray(0, -4)
    : plain;
  const head =
    payload.length < 126
      ? Buffer.from([compressed ? 0xc1 : 0x81, payload.length])
      : Buffer.from([compressed ? 0xc1 : 0x81, 126, payload.length >> 8, payload.length & 255]);
  return Buffer.concat([head, payload]);
}

async function backend(split) {
  const server = createServer();
  const sockets = new Set();
  const offers = [];
  server.on('upgrade', (request, socket) => {
    sockets.add(socket);
    socket.on('error', () => {});
    socket.on('close', () => sockets.delete(socket));
    const compressed = Boolean(request.headers['sec-websocket-extensions']);
    offers.push(request.headers['sec-websocket-extensions']);
    const accept = createHash('sha1')
      .update(request.headers['sec-websocket-key'] + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11')
      .digest('base64');
    const headers = Buffer.from(
      `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n${compressed ? 'Sec-WebSocket-Extensions: permessage-deflate; server_no_context_takeover; client_no_context_takeover\r\n' : ''}\r\n`
    );
    const messages = [
      {
        method: 'Target.attachedToTarget',
        params: { sessionId: 'OBS', targetInfo: { targetId: 'T' } },
      },
      {
        method: 'Page.frameNavigated',
        sessionId: 'OBS',
        params: { frame: { url: 'https://private.test/bfcache?n=upgrade-proof-p' } },
      },
      {
        method: 'Page.frameNavigated',
        sessionId: 'OBS',
        params: { frame: { url: 'https://public.test/bfcache?n=upgrade-proof-b' } },
      },
    ];
    const data = Buffer.concat(messages.map((message) => frame(message, compressed)));
    if (split) {
      socket.write(headers.subarray(0, 31));
      setImmediate(() => socket.write(Buffer.concat([headers.subarray(31), data])));
    } else socket.write(Buffer.concat([headers, data]));
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return {
    url: `ws://127.0.0.1:${server.address().port}`,
    offers,
    close: async () => {
      for (const socket of sockets) socket.destroy();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

for (const split of [false, true])
  test(`upstream gate declines compression and observes ${split ? 'split' : 'coalesced'} upgrade bytes`, async () => {
    const peer = await backend(split);
    const gate = new InboundGate({
      protectedOrigin: 'https://private.test',
      ordinaryOrigin: 'https://public.test',
    });
    const proxy = await cdpProxy(peer.url, { inbound: gate });
    const ws = new WebSocket(proxy.url);
    try {
      await new Promise((resolve, reject) => {
        ws.addEventListener('open', resolve, { once: true });
        ws.addEventListener('error', reject, { once: true });
      });
      assert.deepEqual(
        peer.offers,
        [undefined],
        'proxy must not negotiate a codec it cannot inspect'
      );
      const hook = gate.arm({
        targetId: 'T',
        sessionId: 'OBS',
        ms: 1500,
        side: 'control',
        gate: true,
      });
      assert.equal(hook.protectedObserved, true);
      assert.equal(hook.ordinaryObserved, true);
      gate.close();
    } finally {
      await proxy.close();
      await peer.close();
    }
  });

test('a split client upgrade cannot forward partial extension negotiation upstream', async () => {
  const peer = await backend(false);
  const proxy = await cdpProxy(peer.url);
  const address = new URL(proxy.url);
  const socket = net.connect(Number(address.port), address.hostname);
  let received = '';
  try {
    const answer = new Promise((resolve, reject) => {
      socket.on('data', (bytes) => {
        received += bytes.toString('latin1');
        if (received.includes('\r\n\r\n')) resolve();
      });
      socket.on('error', reject);
    });
    socket.write(
      'GET / HTTP/1.1\r\nHost: fixture.test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: cHVibGljLWZpeHR1cmUta2V5\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Ext'
    );
    await new Promise((resolve) => setImmediate(resolve));
    socket.write('ensions: permessage-deflate; client_max_window_bits\r\n\r\n');
    await answer;
    assert.match(received, /^HTTP\/1\.1 101/);
    assert.deepEqual(peer.offers, [undefined]);
  } finally {
    socket.destroy();
    await proxy.close();
    await peer.close();
  }
});
