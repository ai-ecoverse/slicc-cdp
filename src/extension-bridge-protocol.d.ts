export declare const EXTENSION_BRIDGE_PROTOCOL_VERSION: 1;

export declare const EXTENSION_BRIDGE_PORT_NAME: 'slicc.cdp-bridge';

export interface ExtensionBridgeVersionMismatch {
  bridge: number;
  channelId: string;
  kind: string;
}

export interface ExtensionBridgeHello {
  bridge: typeof EXTENSION_BRIDGE_PROTOCOL_VERSION;
  channelId: string;
  kind: 'handshake.hello';
}

export interface ExtensionBridgeWelcome {
  bridge: typeof EXTENSION_BRIDGE_PROTOCOL_VERSION;
  channelId: string;
  kind: 'handshake.welcome';
}

export interface ExtensionBridgeRejected {
  bridge: typeof EXTENSION_BRIDGE_PROTOCOL_VERSION;
  channelId: string;
  kind: 'handshake.rejected';
  reason: string;
}

export interface ExtensionBridgeCdpRequest {
  bridge: typeof EXTENSION_BRIDGE_PROTOCOL_VERSION;
  channelId: string;
  kind: 'cdp.request';
  id: number;
  method: string;
  params?: Record<string, unknown>;
  sessionId?: string;
}

export interface ExtensionBridgeCdpResponse {
  bridge: typeof EXTENSION_BRIDGE_PROTOCOL_VERSION;
  channelId: string;
  kind: 'cdp.response';
  id: number;
  result?: Record<string, unknown>;
  error?: string;
}

export interface ExtensionBridgeCdpEvent {
  bridge: typeof EXTENSION_BRIDGE_PROTOCOL_VERSION;
  channelId: string;
  kind: 'cdp.event';
  method: string;
  params?: Record<string, unknown>;
  sessionId?: string;
}

export interface ExtensionBridgeLick {
  bridge: typeof EXTENSION_BRIDGE_PROTOCOL_VERSION;
  channelId: string;
  kind: 'extension.lick';
  verb: 'handoff' | 'upskill';
  target: string;
  url: string;
  instruction?: string;
  branch?: string;
  path?: string;
  title?: string;
}

export interface ExtensionBridgeDiscovery {
  bridge: typeof EXTENSION_BRIDGE_PROTOCOL_VERSION;
  channelId: string;
  kind: 'extension.discovery';
  discoveryOrigin: string;
  discoveryKind: 'ai-catalog' | 'llms-txt';
  discoveryUrl: string;
  url: string;
}

export interface ExtensionBridgeLeaderJoinUrl {
  bridge: typeof EXTENSION_BRIDGE_PROTOCOL_VERSION;
  channelId: string;
  kind: 'leader.join-url';
  joinUrl: string | null;
}

export interface ExtensionBridgeOpenSettings {
  bridge: typeof EXTENSION_BRIDGE_PROTOCOL_VERSION;
  channelId: string;
  kind: 'extension.open-settings';
}

export type ExtensionBridgeEnvelope =
  | ExtensionBridgeHello
  | ExtensionBridgeWelcome
  | ExtensionBridgeRejected
  | ExtensionBridgeCdpRequest
  | ExtensionBridgeCdpResponse
  | ExtensionBridgeCdpEvent
  | ExtensionBridgeLick
  | ExtensionBridgeDiscovery
  | ExtensionBridgeLeaderJoinUrl
  | ExtensionBridgeOpenSettings;

export declare function isExtensionBridgeEnvelope(value: unknown): value is ExtensionBridgeEnvelope;

export declare function isBridgeVersionMismatch(
  value: unknown
): value is ExtensionBridgeVersionMismatch;
