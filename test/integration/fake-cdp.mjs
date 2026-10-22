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

const tree = {
  nodes: [
    {
      nodeId: '1',
      role: { type: 'role', value: 'RootWebArea' },
      name: { type: 'computedString', value: 'Fake' },
      childIds: ['2'],
      backendDOMNodeId: 1,
    },
    {
      nodeId: '2',
      parentId: '1',
      role: { type: 'role', value: 'button' },
      name: { type: 'computedString', value: 'Go' },
      childIds: [],
      backendDOMNodeId: 2,
    },
  ],
};

function cookieDomain(domain) {
  return domain.startsWith('.') ? domain : `.${domain}`;
}

function sameCookie(a, b) {
  return a.name === b.name && a.domain === b.domain && a.path === b.path;
}

export async function fakeCdp({
  refuse = [],
  refuseWhen,
  refusalMessage,
  noisyRefusal = false,
  cookies = [],
  traffic = [],
  consoleLines = [],
  answers = {},
  attachEvents = false,
  discoverEvents = false,
  childFrames = [],
  intercept,
} = {}) {
  const calls = [];
  const jar = cookies.map((cookie) => ({ ...cookie }));
  let requests = 0;
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
    const paged = new Set();
    socket.on('close', () => clearInterval(noise));
    const send = (message) => {
      if (!socket.destroyed) socket.write(encode(JSON.stringify(message)));
    };
    const reply = (message) => {
      const { id, method, params = {}, sessionId } = message;
      const answer = (result) => send({ id, result, ...(sessionId ? { sessionId } : {}) });
      const event = (name, eventParams) =>
        send({ method: name, params: eventParams, ...(sessionId ? { sessionId } : {}) });
      const emitConsole = (url) => {
        for (const [index, entry] of consoleLines.entries()) {
          event('Runtime.consoleAPICalled', {
            type: 'log',
            executionContextId: 1,
            timestamp: index + 1,
            args: [{ type: 'string', value: entry.text }],
            stackTrace: { callFrames: [{ url: entry.url ?? url, lineNumber: 0 }] },
          });
        }
      };
      const target = sessionId ? sessions.get(sessionId) : undefined;
      if (intercept?.(message, send)) return;
      if (method === 'Slicc.action') clearInterval(noise);
      if (refuse.includes(method) || refuseWhen?.(message, calls)) {
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
      if (answers[method]) return answer(answers[method]);
      switch (method) {
        case 'Target.setDiscoverTargets':
          answer({});
          if (discoverEvents && params.discover)
            for (const item of targets)
              send({ method: 'Target.targetCreated', params: { targetInfo: info(item) } });
          return;
        case 'Target.getTargets':
          return answer({ targetInfos: targets.map(info) });
        case 'Target.getTargetInfo': {
          const found =
            targets.find((item) => item.targetId === params.targetId) ??
            (childFrames.some((child) => child.frame.id === params.targetId) ? undefined : target);
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
          if (attachEvents)
            event('Target.attachedToTarget', {
              sessionId: session,
              targetInfo: info(found),
              waitingForDebugger: false,
            });
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
          return answer({
            frameTree: {
              frame: target?.frame ?? {
                id: target?.targetId ?? 'F',
                loaderId: 'L',
                url: target?.url ?? '',
                securityOrigin: new URL(target?.url ?? 'about:blank').origin,
              },
              childFrames,
            },
          });
        case 'Page.enable':
          paged.add(sessionId);
          return answer({});
        case 'Page.getNavigationHistory':
          return answer(
            target?.history ?? {
              currentIndex: 0,
              entries: [
                {
                  id: 1,
                  url: target?.url ?? '',
                  userTypedURL: '',
                  title: '',
                  transitionType: 'typed',
                },
              ],
            }
          );
        case 'Page.navigate': {
          if (target) {
            target.url = params.url;
            delete target.frame;
            delete target.history;
          }
          answer({ frameId: target?.targetId ?? 'F', loaderId: `L${next}` });
          const committed = {
            frame: {
              id: target?.targetId ?? 'F',
              loaderId: `L${next}`,
              url: params.url,
              securityOrigin: new URL(params.url).origin,
            },
          };
          for (const [peer, found] of sessions)
            if (target && found === target && peer !== sessionId && paged.has(peer))
              send({ method: 'Page.frameNavigated', params: committed, sessionId: peer });
          event('Page.frameNavigated', committed);
          emitConsole(params.url);
          for (const item of traffic) {
            requests += 1;
            const requestId = `R${requests}`;
            event('Network.requestWillBeSent', {
              requestId,
              loaderId: `L${next}`,
              documentURL: params.url,
              request: { url: item.url, method: 'GET', headers: {} },
              timestamp: 1,
              wallTime: 1,
              initiator: { type: 'other' },
              type: 'Fetch',
            });
            event('Network.requestWillBeSentExtraInfo', {
              requestId,
              associatedCookies: item.associatedCookies ?? [],
              headers: item.requestHeaders,
            });
            event('Network.responseReceived', {
              requestId,
              loaderId: `L${next}`,
              timestamp: 1,
              type: 'Fetch',
              response: {
                url: item.url,
                status: 200,
                statusText: 'OK',
                headers: { 'content-type': 'text/plain' },
                mimeType: 'text/plain',
              },
            });
            event('Network.responseReceivedExtraInfo', {
              requestId,
              blockedCookies: [],
              statusCode: 200,
              headers: item.responseHeaders,
            });
            event('Network.loadingFinished', { requestId, timestamp: 1, encodedDataLength: 0 });
          }
          event('Page.domContentEventFired', { timestamp: 1 });
          event('Page.loadEventFired', { timestamp: 1 });
          return;
        }
        case 'Runtime.enable':
          answer({});
          if (target?.url && target.url !== 'about:blank') emitConsole(target.url);
          return;
        case 'Network.getAllCookies':
          return answer({ cookies: jar.map((cookie) => ({ ...cookie })) });
        case 'Network.setCookie': {
          const cookie = {
            name: params.name,
            value: params.value,
            domain: cookieDomain(params.domain),
            path: params.path ?? '/',
            expires: params.expires ?? -1,
            size: params.name.length + params.value.length,
            httpOnly: params.httpOnly ?? false,
            secure: params.secure ?? false,
            session: params.expires === undefined,
            ...(params.sameSite ? { sameSite: params.sameSite } : {}),
            priority: 'Medium',
            sameParty: false,
            sourceScheme: 'Secure',
            sourcePort: 443,
          };
          const index = jar.findIndex((item) => sameCookie(item, cookie));
          if (index >= 0) jar.splice(index, 1, cookie);
          else jar.push(cookie);
          return answer({ success: true });
        }
        case 'Network.deleteCookies': {
          for (let index = jar.length - 1; index >= 0; index -= 1) {
            const item = jar[index];
            if (
              item.name === params.name &&
              item.domain === params.domain &&
              (!params.path || item.path === params.path) &&
              JSON.stringify(item.partitionKey) === JSON.stringify(params.partitionKey)
            )
              jar.splice(index, 1);
          }
          return answer({});
        }
        case 'Page.createIsolatedWorld':
          return answer({ executionContextId: 7 });
        case 'Accessibility.getFullAXTree':
          return answer(tree);
        case 'DOM.resolveNode':
          return answer({ object: { type: 'object', subtype: 'node', objectId: 'OBJ' } });
        case 'DOM.getBoxModel':
          return answer({
            model: { content: [0, 0, 10, 0, 10, 10, 0, 10], width: 10, height: 10 },
          });
        case 'Runtime.evaluate':
          if (params.expression === 'document.readyState')
            return answer({ result: { type: 'string', value: 'complete' } });
          return answer({ result: { type: 'string', value: 'fake-eval' } });
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
    jar,
    targets,
    close: async () => {
      for (const socket of sockets) socket.destroy();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}
