import { fakeCdp } from './fake-cdp.mjs';

export async function webmcpWire(endpoint) {
  const peers = new Map();
  const gates = [];
  const record = (value) => {
    gates.push(value);
    if (gates.length > 160) gates.shift();
  };
  const proxy = await fakeCdp({
    onDisconnect(send) {
      const peer = peers.get(send);
      if (!peer) return;
      peers.delete(send);
      peer.ws.close();
    },
    intercept(message, send) {
      let peer = peers.get(send);
      if (!peer) {
        const ws = new WebSocket(endpoint);
        peer = {
          ws,
          opened: new Promise((resolve) => ws.addEventListener('open', resolve, { once: true })),
        };
        peers.set(send, peer);
        ws.addEventListener('message', ({ data }) => {
          const value = JSON.parse(data);
          if (value.method === 'Debugger.paused')
            record({ event: value.method, session: value.sessionId, reason: value.params?.reason });
          send(value);
        });
      }
      if (message.method?.startsWith('Debugger.'))
        record({ method: message.method, session: message.sessionId });
      void peer.opened.then(() => peer.ws.send(JSON.stringify(message)));
      return true;
    },
  });
  return {
    url: proxy.url,
    snapshot: () => ({ gates }),
    close: async () => {
      for (const peer of peers.values()) peer.ws.close();
      await proxy.close();
    },
  };
}
