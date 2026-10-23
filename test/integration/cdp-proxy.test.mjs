import assert from 'node:assert/strict';
import { connect } from 'node:net';
import test from 'node:test';
import { cdpProxy } from './cdp-proxy.mjs';
import { fakeCdp } from './fake-cdp.mjs';

for (const split of [false, true]) {
  test(`proxy declines compression negotiation (${split ? 'split' : 'complete'} upgrade)`, {
    timeout: 5000,
  }, async () => {
    let observed;
    const upgraded = new Promise((resolve) => {
      observed = resolve;
    });
    const backend = await fakeCdp({ onUpgrade: observed });
    const proxy = await cdpProxy(backend.url);
    const url = new URL(proxy.url);
    const socket = connect(Number(url.port), url.hostname);
    try {
      await new Promise((resolve, reject) => {
        socket.once('connect', resolve);
        socket.once('error', reject);
      });
      const request = `GET ${url.pathname} HTTP/1.1\r\nHost: ${url.host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Extensions: permessage-deflate\r\n\r\n`;
      if (split) {
        const at = request.indexOf('sions:');
        socket.write(request.slice(0, at));
        await new Promise((resolve) => setTimeout(resolve, 20));
        socket.write(request.slice(at));
      } else socket.write(request);
      assert.equal((await upgraded)['sec-websocket-extensions'], undefined);
    } finally {
      socket.destroy();
      await proxy.close();
      await backend.close();
    }
  });
}
