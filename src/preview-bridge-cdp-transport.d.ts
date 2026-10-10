import type {
  SyntheticCdpTransport,
  SyntheticCdpTransportOptions,
} from './synthetic-cdp-transport.js';
import type { CDPConnectOptions } from './types.js';

export type LeaderToWorkerControlMessage =
  | {
      type: 'bridge.cdp.request';
      connId: string;
      id: number;
      method: string;
      params?: Record<string, unknown>;
      sessionId?: string;
    }
  | {
      type: 'bridge.close';
      connId: string;
    };

export interface PreviewBridgeCdpTransportOptions extends SyntheticCdpTransportOptions {
  connId: string;
  send: (msg: LeaderToWorkerControlMessage) => void;
}

export declare class PreviewBridgeCdpTransport extends SyntheticCdpTransport {
  constructor(opts: PreviewBridgeCdpTransportOptions);
  connect(options?: CDPConnectOptions): Promise<void>;
  disconnect(): void;
  deliverResponse(
    id: number,
    payload: {
      result?: Record<string, unknown>;
      error?: { code: number; message: string };
    }
  ): void;
}
