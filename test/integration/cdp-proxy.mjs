import net from 'node:net';

function frames(buffer) {
  const out = [];
  let offset = 0;
  while (buffer.length - offset >= 2) {
    const second = buffer[offset + 1];
    const masked = (second & 0x80) !== 0;
    let length = second & 0x7f;
    let head = 2;
    if (length === 126) {
      if (buffer.length - offset < 4) break;
      length = buffer.readUInt16BE(offset + 2);
      head = 4;
    } else if (length === 127) {
      if (buffer.length - offset < 10) break;
      length = Number(buffer.readBigUInt64BE(offset + 2));
      head = 10;
    }
    const mask = masked ? 4 : 0;
    const total = head + mask + length;
    if (buffer.length - offset < total) break;
    const raw = buffer.subarray(offset, offset + total);
    const payload = Buffer.from(buffer.subarray(offset + head + mask, offset + total));
    if (masked) {
      const key = buffer.subarray(offset + head, offset + head + 4);
      for (let i = 0; i < payload.length; i += 1) payload[i] ^= key[i % 4];
    }
    out.push({ raw: Buffer.from(raw), opcode: buffer[offset] & 0x0f, payload });
    offset += total;
  }
  return { out, rest: buffer.subarray(offset) };
}

function textFrame(text) {
  const payload = Buffer.from(text);
  const head =
    payload.length < 126
      ? Buffer.from([0x81, payload.length])
      : payload.length < 65536
        ? Buffer.from([0x81, 126, payload.length >> 8, payload.length & 0xff])
        : Buffer.concat([
            Buffer.from([0x81, 127]),
            (() => {
              const size = Buffer.alloc(8);
              size.writeBigUInt64BE(BigInt(payload.length));
              return size;
            })(),
          ]);
  return Buffer.concat([head, payload]);
}

function maskedFrame(text) {
  const plain = textFrame(text);
  const head = plain.length - Buffer.byteLength(text);
  const out = Buffer.concat([plain.subarray(0, head), Buffer.alloc(4), plain.subarray(head)]);
  out[1] |= 0x80;
  return out;
}

function json(frame) {
  if (frame.opcode !== 1) return null;
  try {
    return JSON.parse(frame.payload.toString('utf8'));
  } catch {
    return null;
  }
}

export async function cdpProxy(
  wsUrl,
  {
    delayGuardAttachMs = 0,
    failCloseTarget = false,
    browserEvents = true,
    delays = {},
    early = [],
    transform = (message) => message,
    observe = () => {},
    connectionClosed = () => {},
    forwarded = () => {},
    before = [],
    holdEvents = () => false,
    responseBarrier = () => null,
    inbound,
  } = {}
) {
  const pending = Object.fromEntries(
    Object.entries(delays).map(([method, { ms, times = 1 }]) => [method, { ms, times }])
  );
  const upstream = new URL(wsUrl);
  const replies = [];
  const heldEvents = [];
  const releases = [];
  const sockets = new Set();
  let delayed = false;
  let nextConnection = 0;
  const server = net.createServer((client) => {
    const connection = ++nextConnection;
    sockets.add(client);
    const browser = net.connect(Number(upstream.port), upstream.hostname);
    sockets.add(browser);
    const held = new Set();
    const slowed = new Map();
    const acked = new Set();
    const injected = new Set();
    let nextInjected = 2000000000;
    let heldTarget = null;
    const upgraded = { up: false, down: false };
    let fromClient = Buffer.alloc(0);
    let fromBrowser = Buffer.alloc(0);
    const toClient = (bytes) => {
      if (!client.destroyed) client.write(bytes);
    };
    client.on('data', (chunk) => {
      fromClient = Buffer.concat([fromClient, chunk]);
      if (!upgraded.up) {
        const end = fromClient.indexOf('\r\n\r\n');
        if (end < 0) return;
        const head = fromClient
          .subarray(0, end + 4)
          .toString('latin1')
          .replace(/^sec-websocket-extensions:.*\r\n/gim, '');
        browser.write(Buffer.from(head, 'latin1'));
        fromClient = Buffer.from(fromClient.subarray(end + 4));
        upgraded.up = true;
      }
      const { out, rest } = frames(fromClient);
      fromClient = Buffer.from(rest);
      for (const frame of out) {
        const message = json(frame);
        if (message) {
          observe('client', message, connection);
          inbound?.outbound(message);
        }
        if (
          delayGuardAttachMs &&
          !delayed &&
          message?.method === 'Target.attachToTarget' &&
          !message.sessionId
        ) {
          delayed = true;
          held.add(message.id);
          heldTarget = message.params?.targetId;
        }
        const slow = message?.method ? pending[message.method] : undefined;
        if (slow && slow.times > 0) {
          slow.times -= 1;
          slowed.set(message.id, slow.ms);
        }
        const lead = before.find(
          (rule) =>
            message?.method === rule.method &&
            JSON.stringify(message.params ?? {}).includes(rule.match ?? '')
        );
        if (lead) {
          nextInjected += 1;
          injected.add(nextInjected);
          browser.write(
            maskedFrame(
              JSON.stringify({
                id: nextInjected,
                method: lead.send.method,
                params: lead.send.params ?? {},
                ...(message.sessionId ? { sessionId: message.sessionId } : {}),
              })
            )
          );
        }
        const ack = early.find(
          (rule) =>
            message?.method === rule.method &&
            JSON.stringify(message.params ?? {}).includes(rule.match ?? '')
        );
        if (ack) {
          acked.add(message.id);
          toClient(
            textFrame(
              JSON.stringify({
                id: message.id,
                result: {},
                ...(message.sessionId ? { sessionId: message.sessionId } : {}),
              })
            )
          );
          setTimeout(() => {
            if (!browser.destroyed) browser.write(frame.raw);
          }, ack.ms);
          continue;
        }
        if (failCloseTarget && message?.method === 'Target.closeTarget') {
          toClient(
            textFrame(
              JSON.stringify({
                id: message.id,
                error: { code: -32000, message: 'closeTarget failed' },
              })
            )
          );
          continue;
        }
        browser.write(frame.raw);
        if (message) forwarded(message);
      }
    });
    browser.on('data', (chunk) => {
      fromBrowser = Buffer.concat([fromBrowser, chunk]);
      if (!upgraded.down) {
        const end = fromBrowser.indexOf('\r\n\r\n');
        if (end < 0) return;
        toClient(fromBrowser.subarray(0, end + 4));
        fromBrowser = Buffer.from(fromBrowser.subarray(end + 4));
        upgraded.down = true;
      }
      const { out, rest } = frames(fromBrowser);
      fromBrowser = Buffer.from(rest);
      for (const frame of out) {
        const message = json(frame);
        if (message) observe('browser', message, connection);
        if (message?.method && holdEvents(message)) {
          heldEvents.push(message);
          releases.push(() => toClient(frame.raw));
          continue;
        }
        const barrier = message?.id !== undefined ? responseBarrier(message) : null;
        if (barrier) {
          Promise.resolve(barrier).then(
            () => toClient(frame.raw),
            () => client.destroy()
          );
          continue;
        }
        if (!browserEvents && message?.method && !message.sessionId) continue;
        if (message?.id !== undefined && acked.delete(message.id)) continue;
        if (message?.id !== undefined && injected.delete(message.id)) {
          replies.push(message);
          continue;
        }
        if (message?.id !== undefined && slowed.has(message.id)) {
          const ms = slowed.get(message.id);
          slowed.delete(message.id);
          setTimeout(() => toClient(frame.raw), ms);
          continue;
        }
        const late =
          (message?.id !== undefined && held.has(message.id)) ||
          (message?.method === 'Target.attachedToTarget' &&
            heldTarget &&
            message.params?.targetInfo?.targetId === heldTarget &&
            held.size > 0);
        if (late) {
          if (message.id !== undefined) held.delete(message.id);
          setTimeout(() => toClient(frame.raw), delayGuardAttachMs);
          continue;
        }
        const deliver = () =>
          toClient(message ? textFrame(JSON.stringify(transform(message))) : frame.raw);
        if (inbound) inbound.inbound(message, deliver);
        else deliver();
      }
    });
    let closed = false;
    const end = () => {
      if (closed) return;
      closed = true;
      connectionClosed(connection);
      inbound?.close();
      client.destroy();
      browser.destroy();
    };
    client.on('close', end);
    browser.on('close', end);
    client.on('error', end);
    browser.on('error', end);
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return {
    url: `ws://127.0.0.1:${server.address().port}${upstream.pathname}`,
    replies,
    heldEvents,
    releaseEvents: () => {
      for (const release of releases.splice(0)) release();
    },
    close: async () => {
      inbound?.close();
      for (const socket of sockets) socket.destroy();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}
