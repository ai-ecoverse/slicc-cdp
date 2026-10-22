import { createHash } from 'node:crypto';
import { cdpProxy } from './cdp-proxy.mjs';

const safeErrors = new Set([
  "slicc: refused: target is on seven's origin\n",
  'bridge evaluation navigated or superseded',
  'bridge evaluation timeout',
  'bridge document identity unavailable',
  'Cannot find context with specified id',
  'Execution context was destroyed.',
]);

export function diagnosticValue(value, key = '') {
  if (typeof value === 'string') {
    if (key === 'method' && /^[A-Z][A-Za-z]*\.[A-Za-z]+$/.test(value)) return value;
    if (key === 'phase' && ['original', 'poll', 'superseded'].includes(value)) return value;
    if (key === 'side' && ['control', 'guarded'].includes(value)) return value;
    if (key === 'message') {
      return {
        kind: /refused/.test(value)
          ? 'refusal'
          : /navigated or superseded/.test(value)
            ? 'superseded'
            : /context.*(destroyed|specified id)/i.test(value)
              ? 'context'
              : 'error',
        ...(safeErrors.has(value) ? { text: value } : {}),
        sha256: createHash('sha256').update(value).digest('hex'),
        bytes: Buffer.byteLength(value),
      };
    }
    return {
      sha256: createHash('sha256').update(value).digest('hex'),
      bytes: Buffer.byteLength(value),
    };
  }
  if (Array.isArray(value)) return value.map((item) => diagnosticValue(item));
  if (value && typeof value === 'object')
    return Object.fromEntries(
      Object.entries(value).map(([name, item]) => [name, diagnosticValue(item, name)])
    );
  return value;
}

export async function guardDiagnostics(wsUrl, metadata) {
  const record = { ...metadata, calls: [], events: [], wire: [], bridge: [], snapshots: [] };
  const methods = new Map();
  const proxy = await cdpProxy(wsUrl, {
    observe: (direction, message) => {
      if (direction === 'client' && message.id !== undefined)
        methods.set(message.id, message.method);
      record.wire.push({
        at: Date.now(),
        direction,
        ...(direction === 'browser' && message.id !== undefined
          ? { responseTo: methods.get(message.id) }
          : {}),
        message: diagnosticValue(message),
      });
    },
  });
  return {
    record,
    url: proxy.url,
    onMessage: (message) => {
      if (message.event)
        record.events.push({ at: Date.now(), event: diagnosticValue(message.event) });
      if (message.diagnostic)
        record.bridge.push({ at: Date.now(), ...diagnosticValue(message.diagnostic) });
    },
    wrap: (client) => {
      const send = client.send;
      client.send = async (method, params = {}, sessionId) => {
        const call = {
          id: record.calls.length + 1,
          at: Date.now(),
          method,
          params: diagnosticValue(params),
          sessionId: diagnosticValue(sessionId),
        };
        record.calls.push(call);
        try {
          const result = await send(method, params, sessionId);
          call.result = diagnosticValue(result);
          return result;
        } catch (error) {
          call.error = diagnosticValue(error.cdp ?? { message: error.message });
          throw error;
        } finally {
          call.finished = Date.now();
        }
      };
    },
    snapshot: (details) => record.snapshots.push({ at: Date.now(), ...diagnosticValue(details) }),
    close: proxy.close,
  };
}
