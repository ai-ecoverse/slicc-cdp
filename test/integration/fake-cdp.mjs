import { createHash } from 'node:crypto';
import { createServer } from 'node:http';

function decode(buffer) {
  const out = [];
  let offset = 0;
  while (buffer.length - offset >= 2) {
    const second = buffer[offset + 1];
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
    const mask = second & 0x80 ? 4 : 0;
    const total = head + mask + length;
    if (buffer.length - offset < total) break;
    const payload = Buffer.from(buffer.subarray(offset + head + mask, offset + total));
    if (mask) {
      const key = buffer.subarray(offset + head, offset + head + 4);
      for (let i = 0; i < payload.length; i += 1) payload[i] ^= key[i % 4];
    }
    out.push({ opcode: buffer[offset] & 0x0f, payload });
    offset += total;
  }
  return { out, rest: buffer.subarray(offset) };
}

function encode(text, opcode = 1) {
  const payload = Buffer.from(text);
  let head;
  if (payload.length < 126) head = Buffer.from([0x80 | opcode, payload.length]);
  else if (payload.length < 65536) {
    head = Buffer.from([0x80 | opcode, 126, 0, 0]);
    head.writeUInt16BE(payload.length, 2);
  } else {
    head = Buffer.alloc(10);
    head[0] = 0x80 | opcode;
    head[1] = 127;
    head.writeBigUInt64BE(BigInt(payload.length), 2);
  }
  return Buffer.concat([head, payload]);
}

const defaultAria = {
  json: [
    {
      role: 'generic',
      ref: 'e1',
      children: [{ role: 'button', name: 'Go', ref: 'e2', cursor: 'pointer' }],
    },
  ],
  refs: [
    { ref: 'e1', selector: ':root > :nth-child(2)' },
    { ref: 'e2', selector: ':root > :nth-child(2) > :nth-child(1)' },
  ],
  base: 'http://example.test/',
  nodes: [{ backendNodeId: 1 }, { backendNodeId: 2 }],
};

function ariaOptions(expression) {
  const match = expression.match(/\nsliccAriaSnapshot\((\{.*\})\)$/);
  return match ? JSON.parse(match[1]) : undefined;
}

function ariaAnswer(capture, location, expression) {
  if (capture?.exception) {
    return { result: { type: 'object' }, exceptionDetails: { text: capture.exception } };
  }
  if (capture) {
    const { json, refs, base, iframeDepths } = capture;
    return { result: { type: 'object', value: { json, refs, base, iframeDepths } } };
  }
  if (expression.startsWith('globalThis[')) {
    return { result: { type: 'object', subtype: 'array', objectId: 'ARIA' } };
  }
  if (location && expression.startsWith('JSON.stringify({href:location.href')) {
    return { result: { type: 'string', value: JSON.stringify(location) } };
  }
  return undefined;
}

export async function fakeCdp({
  refuse = [],
  refusalMessage,
  noisyRefusal = false,
  aria = () => defaultAria,
  location,
  frameTree,
} = {}) {
  let lastAria = defaultAria;
  const calls = [];
  const targets = [];
  const sessions = new Map();
  let next = 0;
  let tabs = 0;
  const info = (target) => ({
    targetId: target.targetId,
    type: 'page',
    url: target.url,
    title: target.url,
    attached: false,
    canAccessOpener: false,
    browserContextId: 'CTX',
  });
  const server = createServer((req, res) => {
    res.writeHead(404);
    res.end();
  });
  const sockets = new Set();
  server.on('upgrade', (req, socket) => {
    sockets.add(socket);
    socket.on('close', () => sockets.delete(socket));
    socket.on('error', () => socket.destroy());
    const accept = createHash('sha1')
      .update(`${req.headers['sec-websocket-key']}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
      .digest('base64');
    socket.write(
      `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`
    );
    let noise;
    socket.on('close', () => clearInterval(noise));
    const send = (message) => {
      if (!socket.destroyed) socket.write(encode(JSON.stringify(message)));
    };
    const reply = (message) => {
      const { id, method, params = {}, sessionId } = message;
      const answer = (result) => send({ id, result, ...(sessionId ? { sessionId } : {}) });
      const event = (name, eventParams) =>
        send({ method: name, params: eventParams, ...(sessionId ? { sessionId } : {}) });
      const target = sessionId ? sessions.get(sessionId) : undefined;
      if (method === 'Slicc.action') clearInterval(noise);
      if (refuse.includes(method)) {
        send({ id, error: { code: -32000, message: refusalMessage ?? `${method} refused` } });
        if (noisyRefusal) {
          noise = setInterval(
            () =>
              send({
                method: 'Target.targetInfoChanged',
                params: {
                  targetInfo: {
                    targetId: 'NOISY',
                    type: 'page',
                    url: 'https://example.test/',
                    title: 'Activity',
                  },
                },
              }),
            20
          );
        }
        return;
      }
      switch (method) {
        case 'Target.getTargets':
          return answer({ targetInfos: targets.map(info) });
        case 'Target.getTargetInfo': {
          const found = targets.find((item) => item.targetId === params.targetId) ?? target;
          if (!found) return send({ id, error: { code: -32602, message: 'No target' } });
          return answer({ targetInfo: info(found) });
        }
        case 'Target.createTarget': {
          tabs += 1;
          const created = { targetId: `TAB${tabs}`, url: params.url ?? 'about:blank' };
          targets.push(created);
          return answer({ targetId: created.targetId });
        }
        case 'Target.attachToTarget': {
          const found = targets.find((item) => item.targetId === params.targetId);
          if (!found) return send({ id, error: { code: -32602, message: 'No target' } });
          next += 1;
          const session = `S${next}`;
          sessions.set(session, found);
          return answer({ sessionId: session });
        }
        case 'Target.detachFromTarget':
          sessions.delete(params.sessionId);
          return answer({});
        case 'Target.closeTarget': {
          const index = targets.findIndex((item) => item.targetId === params.targetId);
          if (index >= 0) targets.splice(index, 1);
          return answer({ success: index >= 0 });
        }
        case 'Browser.getVersion':
          return answer({ product: 'Chrome/140.0.0.0', protocolVersion: '1.3' });
        case 'Page.getFrameTree':
          if (frameTree) return answer({ frameTree });
          return answer({
            frameTree: {
              frame: { id: target?.targetId ?? 'F', loaderId: 'L', url: target?.url ?? '' },
            },
          });
        case 'Page.navigate': {
          if (target) target.url = params.url;
          answer({ frameId: target?.targetId ?? 'F', loaderId: `L${next}` });
          event('Page.frameNavigated', {
            frame: { id: target?.targetId ?? 'F', loaderId: `L${next}`, url: params.url },
          });
          event('Page.domContentEventFired', { timestamp: 1 });
          event('Page.loadEventFired', { timestamp: 1 });
          return;
        }
        case 'Page.createIsolatedWorld':
          return answer({ executionContextId: 7 });
        case 'Runtime.getProperties':
          return answer({
            result: lastAria.nodes.flatMap((node, index) =>
              node
                ? [
                    {
                      name: String(index),
                      value: { type: 'object', subtype: 'node', objectId: `NODE${index}` },
                    },
                  ]
                : []
            ),
          });
        case 'DOM.describeNode': {
          const index = Number(String(params.objectId).replace('NODE', ''));
          return answer({ node: { nodeType: 1, ...lastAria.nodes[index] } });
        }
        case 'DOM.resolveNode':
          return answer({ object: { type: 'object', subtype: 'node', objectId: 'OBJ' } });
        case 'DOM.getBoxModel':
          return answer({
            model: { content: [0, 0, 10, 0, 10, 10, 0, 10], width: 10, height: 10 },
          });
        case 'Runtime.evaluate': {
          const options = ariaOptions(params.expression ?? '');
          const capture = options ? aria(options, params) : undefined;
          if (capture?.error) return send({ id, error: { code: -32000, message: capture.error } });
          if (!capture && lastAria.handle && params.expression?.startsWith('globalThis[')) {
            if (lastAria.handle === 'error')
              return send({ id, error: { code: -32000, message: 'handle refused' } });
            return answer({ result: { type: 'undefined' } });
          }
          if (capture) lastAria = capture;
          const answered = ariaAnswer(capture, location, params.expression ?? '');
          if (answered) return answer(answered);
          if (params.expression === 'document.readyState')
            return answer({ result: { type: 'string', value: 'complete' } });
          return answer({ result: { type: 'string', value: 'fake-eval' } });
        }
        case 'Runtime.callFunctionOn':
          return answer({
            result: { type: 'object', value: { x: 5, y: 5, width: 10, height: 10 } },
          });
        default:
          return answer({});
      }
    };
    let pending = Buffer.alloc(0);
    socket.on('data', (chunk) => {
      pending = Buffer.concat([pending, chunk]);
      const { out, rest } = decode(pending);
      pending = Buffer.from(rest);
      for (const frame of out) {
        if (frame.opcode === 8) {
          socket.end(encode('', 8));
          continue;
        }
        if (frame.opcode !== 1) continue;
        const message = JSON.parse(frame.payload.toString('utf8'));
        calls.push({ ...message, target: sessions.get(message.sessionId)?.targetId });
        reply(message);
      }
    });
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return {
    url: `ws://127.0.0.1:${server.address().port}/devtools/browser/fake`,
    calls,
    close: async () => {
      for (const socket of sockets) socket.destroy();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}
