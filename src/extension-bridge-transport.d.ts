import type { CdpTransportBridge } from './cdp-bridge.js';
import type { ExtensionBridgeDiscovery, ExtensionBridgeLick } from './extension-bridge-protocol.js';
import type { CDPConnectOptions } from './types.js';

export interface ExtensionBridgePort {
  postMessage(message: unknown): void;
  disconnect(): void;
  onMessage: { addListener(cb: (msg: unknown) => void): void };
  onDisconnect: { addListener(cb: () => void): void };
}

export interface ExtensionBridgeTransportOptions {
  extensionId: string;
  connect?: (extensionId: string, info: { name: string }) => ExtensionBridgePort;
  handshakeTimeoutMs?: number;
  onLick?: (lick: ExtensionBridgeLick) => void;
  onDiscovery?: (discovery: ExtensionBridgeDiscovery) => void;
  onOpenSettings?: () => void;
}

export declare class ExtensionBridgeTransport extends CdpTransportBridge {
  readonly isExtensionBridge: true;
  constructor(opts: ExtensionBridgeTransportOptions);
  connect(options?: CDPConnectOptions): Promise<void>;
  sendLeaderJoinUrl(joinUrl: string | null): void;
  disconnect(): void;
  testReceive(raw: unknown): void;
}
