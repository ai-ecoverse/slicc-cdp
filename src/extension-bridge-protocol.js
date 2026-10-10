export const EXTENSION_BRIDGE_PROTOCOL_VERSION = 1;

export const EXTENSION_BRIDGE_PORT_NAME = 'slicc.cdp-bridge';

const KINDS = new Set([
  'handshake.hello',
  'handshake.welcome',
  'handshake.rejected',
  'cdp.request',
  'cdp.response',
  'cdp.event',
  'extension.lick',
  'extension.discovery',
  'leader.join-url',
  'extension.open-settings',
]);

export function isExtensionBridgeEnvelope(value) {
  if (typeof value !== 'object' || value === null) return false;
  const record = value;
  return (
    record.bridge === EXTENSION_BRIDGE_PROTOCOL_VERSION &&
    typeof record.channelId === 'string' &&
    typeof record.kind === 'string' &&
    KINDS.has(record.kind)
  );
}

export function isBridgeVersionMismatch(value) {
  if (typeof value !== 'object' || value === null) return false;
  const record = value;
  return (
    typeof record.bridge === 'number' &&
    record.bridge !== EXTENSION_BRIDGE_PROTOCOL_VERSION &&
    typeof record.channelId === 'string' &&
    typeof record.kind === 'string'
  );
}
