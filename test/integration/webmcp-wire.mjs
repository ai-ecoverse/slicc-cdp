import { fakeCdp } from './fake-cdp.mjs';

export async function webmcpWire(endpoint) {
  const peers = new Map();
  const recent = [];
  const gates = [];
  const responses = [];
  const probes = new Map();
  const record = (entry) => {
    entry.at = Date.now();
    recent.push(entry);
    if (recent.length > 120) recent.shift();
    if (
      entry.event ||
      /Debugger\.(resume|evaluateOnCallFrame)|Page\.(addScript|removeScript)|Runtime\.runIfWaiting|Fetch\./.test(
        entry.method || entry.reply
      )
    ) {
      gates.push(entry);
      if (gates.length > 160) gates.shift();
    }
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
          commands: new Map(),
          sessions: new Map(),
          contexts: new Map(),
          scripts: new Map(),
          pauses: new Map(),
          fetches: new Map(),
          opened: new Promise((resolve) => ws.addEventListener('open', resolve, { once: true })),
        };
        peers.set(send, peer);
        ws.addEventListener('message', ({ data }) => {
          const value = JSON.parse(data);
          const session = value.sessionId;
          const params = value.params || {};
          const key = (id) => `${session}:${id}`;
          const command = peer.commands.get(value.id);
          if (command) {
            peer.commands.delete(value.id);
            if (command.method === 'Target.attachToTarget' && value.result?.sessionId) {
              peer.sessions.set(value.result.sessionId, command.target);
            }
            record({ reply: command.method, session: command.session, error: !!value.error });
            if (command.counter) {
              const answer = value.result?.result?.value;
              probes.set(command.session, {
                session: command.session,
                value:
                  typeof answer === 'string' && /^b\d+$/.test(answer)
                    ? answer
                    : answer === ''
                      ? 'empty'
                      : 'other',
                exception: !!value.result?.exceptionDetails,
              });
            }
          }
          if (value.method === 'Target.attachedToTarget') {
            peer.sessions.set(params.sessionId, params.targetInfo.targetId);
            record({
              event: value.method,
              session,
              child: params.sessionId,
              type: params.targetInfo.type,
            });
          }
          if (value.method === 'Network.responseReceived' && params.type === 'Document') {
            const headers = params.response?.headers || {};
            responses.push({
              session,
              frame: params.frameId,
              loader: params.loaderId,
              policyObserved: true,
              explicitTools: Object.entries(headers).some(
                ([name, value]) =>
                  name.toLowerCase() === 'permissions-policy' && String(value).includes('tools')
              ),
            });
            if (responses.length > 80) responses.shift();
          }
          if (value.method === 'Runtime.executionContextCreated') {
            peer.contexts.set(key(params.context.id), params.context.auxData?.frameId);
          }
          if (value.method === 'Debugger.scriptParsed') {
            peer.scripts.set(key(params.scriptId), params.executionContextId);
          }
          if (value.method === 'Debugger.paused') {
            const script = params.callFrames?.[0]?.location?.scriptId;
            const frame = peer.contexts.get(key(peer.scripts.get(key(script))));
            const target = peer.sessions.get(session);
            const pause = { event: value.method, session, target, frame, reason: params.reason };
            peer.pauses.set(session, pause);
            record(pause);
          }
          if (value.method === 'Debugger.resumed') peer.pauses.delete(session);
          if (value.method === 'Fetch.requestPaused') {
            const pause = {
              event: value.method,
              session,
              frame: params.frameId,
              request: params.requestId,
              type: params.resourceType,
              response: !!params.responseStatusCode,
            };
            peer.fetches.set(key(params.requestId), pause);
            record(pause);
          }
          if (
            [
              'Page.frameNavigated',
              'Page.frameDetached',
              'Page.loadEventFired',
              'Target.detachedFromTarget',
            ].includes(value.method)
          ) {
            record({
              event: value.method,
              session,
              frame: params.frame?.id || params.frameId,
              parent: params.frame?.parentId,
              loader: params.frame?.loaderId,
              child: params.sessionId,
              reason: params.reason,
            });
          }
          send(value);
        });
      }
      const command = {
        method: message.method,
        session: message.sessionId,
        target: message.params?.targetId,
        immediate: message.params?.runImmediately,
        counter: message.params?.expression?.includes('listed.changes') || undefined,
      };
      peer.commands.set(message.id, command);
      if (message.method.startsWith('Fetch.') && message.params?.requestId) {
        peer.fetches.delete(`${message.sessionId}:${message.params.requestId}`);
      }
      record(command);
      void peer.opened.then(() => peer.ws.send(JSON.stringify(message)));
      return true;
    },
  });
  return {
    url: proxy.url,
    snapshot: () => ({
      recent,
      gates,
      responses,
      probes: [...probes.values()],
      pending: [...peers.values()].flatMap((peer) => [...peer.commands.values()]),
      pauses: [...peers.values()].flatMap((peer) => [...peer.pauses.values()]),
      fetches: [...peers.values()].flatMap((peer) => [...peer.fetches.values()]),
    }),
    close: async () => {
      for (const peer of peers.values()) peer.ws.close();
      await proxy.close();
    },
  };
}
