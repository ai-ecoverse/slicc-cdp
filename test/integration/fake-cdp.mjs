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

const LANDING = "Symbol.for('slicc-landing')";

function landed() {
  let armed = ['landed'];
  return (mode, args) => {
    if (mode === 'arm') armed = args.types;
    return { arm: 'armed', read: armed, visible: 'visible', focus: 'editable' }[mode] ?? 'done';
  };
}

function landingCall(params) {
  if (params.functionDeclaration?.includes(LANDING))
    return [params.arguments?.[0]?.value, params.arguments?.[1]?.value];
  const match = params.expression?.includes(LANDING)
    ? params.expression.match(/\("(\w+)", (\{.*\})\)$/s)
    : null;
  return match ? [match[1], JSON.parse(match[2])] : undefined;
}

function cookieDomain(domain) {
  return domain.startsWith('.') ? domain : `.${domain}`;
}

function samePartition(a, b) {
  return a?.topLevelSite === b?.topLevelSite && a?.hasCrossSiteAncestor === b?.hasCrossSiteAncestor;
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
  tabSessions = false,
  discoverEvents = false,
  delayHistoryEvents = false,
  onUpgrade,
  onDisconnect,
  onCommand,
  pauseNavigation = false,
  childFrames = [],
  intercept,
  aria = () => defaultAria,
  landing = landed(),
  visibility,
  windows = () => ({ windowId: 1, bounds: { windowState: 'normal' } }),
  tree = { nodes: [] },
  location,
  frameTree,
  listen = '127.0.0.1',
  version = false,
} = {}) {
  let lastAria = defaultAria;
  const calls = [];
  const navigationPauses = new Map();
  const fetchSessions = new Set();
  const jar = cookies.map((cookie) => ({ ...cookie }));
  let requests = 0;
  const targets = [];
  const sessions = new Map();
  const heldEvents = [];
  const releaseEvents = [];
  const sessionSends = new Map();
  const paged = new Set();
  let next = 0;
  let tabs = 0;
  let loaders = 0;
  const info = (target) => ({
    targetId: target.targetId,
    type: target.type ?? 'page',
    ...(target.openerId ? { openerId: target.openerId } : {}),
    url: target.url,
    title: target.url,
    attached: false,
    canAccessOpener: false,
    browserContextId: 'CTX',
    ...(target.parentFrameId ? { parentFrameId: target.parentFrameId } : {}),
  });
  const server = createServer((req, res) => {
    if (version && req.url === '/json/version') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(
        JSON.stringify({
          Browser: 'Chrome/140.0.0.0',
          'Protocol-Version': '1.3',
          webSocketDebuggerUrl: `ws://${req.headers.host}/devtools/browser/fake`,
        })
      );
      return;
    }
    res.writeHead(404);
    res.end();
  });
  const sockets = new Set();
  server.on('upgrade', (req, socket) => {
    onUpgrade?.(req.headers);
    sockets.add(socket);
    socket.on('close', () => {
      onDisconnect?.(send);
      sockets.delete(socket);
      for (const [session, owner] of sessionSends) {
        if (owner !== send) continue;
        sessions.delete(session);
        sessionSends.delete(session);
        paged.delete(session);
      }
    });
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
      onCommand?.(message, event);
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
          const url = params.url ?? 'about:blank';
          const created = {
            targetId: `TAB${tabs}`,
            url,
            loaderId: `L${++loaders}`,
          };
          targets.push(created);
          return answer({ targetId: created.targetId });
        }
        case 'Target.attachToTarget': {
          const found = targets.find((item) => item.targetId === params.targetId);
          if (!found) return send({ id, error: { code: -32602, message: 'No target' } });
          next += 1;
          let session = `S${next}`;
          if (tabSessions) {
            session = found.targetId;
            for (let n = 1; sessions.has(session); n += 1) session = `${found.targetId}.${n}`;
          }
          sessions.set(session, found);
          sessionSends.set(session, send);
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
          sessionSends.delete(params.sessionId);
          paged.delete(params.sessionId);
          return answer({});
        case 'Target.closeTarget': {
          const index = targets.findIndex((item) => item.targetId === params.targetId);
          if (index >= 0) targets.splice(index, 1);
          return answer({ success: index >= 0 });
        }
        case 'Browser.getWindowForTarget':
          return answer(windows(params.targetId ?? target?.targetId));
        case 'Browser.getVersion':
          return answer({ product: 'Chrome/140.0.0.0', protocolVersion: '1.3' });
        case 'Page.getFrameTree':
          if (target?.tree) return answer({ frameTree: target.tree });
          if (frameTree) return answer({ frameTree });
          return answer({
            frameTree: {
              frame: target?.frame ?? {
                id: target?.targetId ?? 'F',
                loaderId: target?.loaderId ?? 'L',
                url: target?.url ?? '',
                securityOrigin:
                  target?.url && new URL(target.url).origin !== 'null'
                    ? new URL(target.url).origin
                    : '://',
              },
              childFrames: target?.childFrames ?? childFrames,
            },
          });
        case 'Fetch.enable':
          fetchSessions.add(sessionId);
          return answer({});
        case 'Fetch.disable':
          fetchSessions.delete(sessionId);
          return answer({});
        case 'Fetch.fulfillRequest':
        case 'Fetch.continueRequest': {
          const complete = navigationPauses.get(params.requestId);
          navigationPauses.delete(params.requestId);
          complete?.();
          return answer({});
        }
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
          const complete = () => {
            const loaderId = `L${++loaders}`;
            const frame = {
              id: target?.frame?.id ?? target?.targetId ?? 'F',
              loaderId,
              url: params.url,
              securityOrigin: new URL(params.url).origin,
            };
            if (target) {
              target.url = params.url;
              target.loaderId = loaderId;
              target.frame = frame;
              delete target.history;
            }
            answer({ frameId: frame.id, loaderId });
            const committed = { frame };
            for (const [peer, found] of sessions)
              if (target && found === target && peer !== sessionId && paged.has(peer))
                sessionSends.get(peer)?.({
                  method: 'Page.frameNavigated',
                  params: committed,
                  sessionId: peer,
                });
            event('Page.frameNavigated', committed);
            emitConsole(params.url);
            for (const item of traffic) {
              requests += 1;
              const requestId = `R${requests}`;
              event('Network.requestWillBeSent', {
                requestId,
                loaderId,
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
                loaderId,
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
          };
          if (pauseNavigation && fetchSessions.has(sessionId)) {
            const requestId = `NAV-${sessionId}`;
            navigationPauses.set(requestId, complete);
            event('Fetch.requestPaused', {
              requestId,
              networkId: `NET-${sessionId}`,
              frameId: target?.targetId ?? 'F',
              resourceType: 'Document',
              request: { url: params.url, method: 'GET', headers: {} },
            });
          } else complete();
          return;
        }
        case 'Page.printToPDF':
          return answer({ stream: 'H' });
        case 'Runtime.enable': {
          const frame = target?.frame ?? {
            id: target?.targetId ?? 'F',
            loaderId: target?.loaderId ?? 'L',
            securityOrigin:
              target?.url && new URL(target.url).origin !== 'null'
                ? new URL(target.url).origin
                : '://',
          };
          event('Runtime.executionContextCreated', {
            context: {
              id: 1,
              uniqueId: `default:${target?.targetId}:${frame.loaderId}`,
              origin: frame.securityOrigin,
              name: '',
              auxData: { frameId: frame.id, isDefault: true },
            },
          });
          for (const world of target?.worlds ?? [])
            event('Runtime.executionContextCreated', { context: world });
          answer({});
          if (target?.url && target.url !== 'about:blank') emitConsole(target.url);
          return;
        }
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
              samePartition(item.partitionKey, params.partitionKey)
            )
              jar.splice(index, 1);
          }
          return answer({});
        }
        case 'Page.createIsolatedWorld': {
          const worlds = target?.worlds ?? [];
          const previous = worlds.find((world) => world.auxData.frameId === params.frameId);
          const world = {
            id: previous?.id ?? 7 + worlds.length,
            uniqueId: `world:${params.frameId}:${target?.loaderId ?? 'L'}`,
            origin:
              target?.url && new URL(target.url).origin !== 'null'
                ? new URL(target.url).origin
                : '://',
            name: params.worldName ?? '',
            auxData: { frameId: params.frameId, isDefault: false },
          };
          if (target)
            target.worlds = [
              ...worlds.filter((item) => item.auxData.frameId !== params.frameId),
              world,
            ];
          return answer({ executionContextId: world.id });
        }
        case 'Accessibility.getFullAXTree':
          return answer(tree);
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
          const call = landingCall(params);
          const landed = call && landing(...call, target);
          if (landed?.exception)
            return answer({
              result: { type: 'object' },
              exceptionDetails: { text: landed.exception },
            });
          if (call) return answer({ result: { type: 'string', value: landed } });
          if (visibility && params.expression === 'document.visibilityState')
            return answer({ result: { type: 'string', value: visibility(target) } });
          if (params.expression === 'history.back()' && target?.history?.currentIndex > 0) {
            target.history.currentIndex -= 1;
            target.url = target.history.entries[target.history.currentIndex].url;
            target.frame = {
              id: target.targetId,
              loaderId: 'LHISTORY',
              url: target.url,
              securityOrigin: new URL(target.url).origin,
            };
            for (const [peer, found] of sessions) {
              if (found !== target || !paged.has(peer)) continue;
              const restored = {
                method: 'Page.frameNavigated',
                sessionId: peer,
                params: { type: 'BackForwardCacheRestore', frame: target.frame },
              };
              if (delayHistoryEvents) {
                heldEvents.push(restored);
                releaseEvents.push(() => send(restored));
              } else send(restored);
            }
            return answer({ result: { type: 'undefined' } });
          }
          const options = ariaOptions(params.expression ?? '');
          const capture = options ? aria(options, params, target) : undefined;
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
          if (landingCall(params))
            return answer({
              result: { type: 'string', value: landing(...landingCall(params), target) },
            });
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
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, listen, resolve);
  });
  const host = listen.includes(':') ? `[${listen}]` : listen;
  return {
    url: `ws://${host}:${server.address().port}/devtools/browser/fake`,
    port: server.address().port,
    calls,
    jar,
    targets,
    heldEvents,
    releaseEvents: () => {
      for (const release of releaseEvents.splice(0)) release();
    },
    close: async () => {
      for (const socket of sockets) socket.destroy();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}
