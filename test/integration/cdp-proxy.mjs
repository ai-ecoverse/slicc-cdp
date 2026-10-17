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
    before = [],
  } = {}
) {
  const pending = Object.fromEntries(
    Object.entries(delays).map(([method, { ms, times = 1 }]) => [method, { ms, times }])
  );
  const upstream = new URL(wsUrl);
  const replies = [];
  const sockets = new Set();
  let delayed = false;
  const server = net.createServer((client) => {
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
      if (!upgraded.up) {
        upgraded.up = true;
        browser.write(chunk);
        return;
      }
      fromClient = Buffer.concat([fromClient, chunk]);
      const { out, rest } = frames(fromClient);
      fromClient = Buffer.from(rest);
      for (const frame of out) {
        const message = json(frame);
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
      }
    });
    browser.on('data', (chunk) => {
      if (!upgraded.down) {
        upgraded.down = true;
        toClient(chunk);
        return;
      }
      fromBrowser = Buffer.concat([fromBrowser, chunk]);
      const { out, rest } = frames(fromBrowser);
      fromBrowser = Buffer.from(rest);
      for (const frame of out) {
        const message = json(frame);
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
        toClient(frame.raw);
      }
    });
    const end = () => {
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
    close: async () => {
      for (const socket of sockets) socket.destroy();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}
