export const CHERRY_PROTOCOL_VERSION = 2;

export const SUPPORTED_CHERRY_PROTOCOL_VERSIONS = [2, 1];

const KINDS = new Set([
  'handshake.hello',
  'handshake.welcome',
  'handshake.version-mismatch',
  'cdp.request',
  'cdp.response',
  'cdp.event',
  'permission.request',
  'permission.response',
  'host.event',
  'slicc.event',
  'session.export.request',
  'session.export.cancel',
  'session.export.progress',
  'session.export.response',
  'session.export.error',
]);

const EXPORT_KINDS = new Set([
  'session.export.request',
  'session.export.cancel',
  'session.export.progress',
  'session.export.response',
  'session.export.error',
]);

function exportEnvelopeFieldsHold(record) {
  if (typeof record.requestId !== 'string' || record.requestId === '') return false;
  if (record.kind === 'session.export.progress' && typeof record.phase !== 'string') return false;
  if (record.kind === 'session.export.response' && !(record.blob instanceof Blob)) return false;
  if (record.kind === 'session.export.error' && typeof record.code !== 'string') return false;
  return true;
}

export function isCherryEnvelope(value, versions = [CHERRY_PROTOCOL_VERSION]) {
  if (typeof value !== 'object' || value === null) return false;
  const record = value;
  if (
    typeof record.cherry !== 'number' ||
    !versions.includes(record.cherry) ||
    typeof record.channelId !== 'string' ||
    typeof record.kind !== 'string' ||
    !KINDS.has(record.kind)
  ) {
    return false;
  }
  if (record.kind === 'handshake.version-mismatch' && typeof record.peerVersion !== 'number') {
    return false;
  }
  if (EXPORT_KINDS.has(record.kind) && !exportEnvelopeFieldsHold(record)) return false;
  return true;
}

export function acceptEnvelope(event, ctx) {
  if (!ctx.allowOrigins.includes(event.origin)) return false;
  if (ctx.expectedSource !== null && event.source !== ctx.expectedSource) return false;
  if (!isCherryEnvelope(event.data, ctx.versions)) return false;
  if (ctx.channelId !== null && event.data.channelId !== ctx.channelId) return false;
  return true;
}

export function isCherryVersionMismatch(value, supported = [CHERRY_PROTOCOL_VERSION]) {
  if (typeof value !== 'object' || value === null) return false;
  const record = value;
  return (
    typeof record.cherry === 'number' &&
    !supported.includes(record.cherry) &&
    typeof record.channelId === 'string' &&
    typeof record.kind === 'string'
  );
}
