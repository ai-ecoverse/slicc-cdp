import type { SyntheticCdpTransport } from './synthetic-cdp-transport.js';
import type { CDPConnectOptions } from './types.js';

export interface CherryHostTransportOptions {
  counterpart: Window;
  allowOrigins: string[];
  targetOrigin: string;
  capabilities?: { navigate: boolean; screenshot: boolean; openUrl: boolean };
}

export interface CherryFeatures {
  terminal: boolean;
  files: boolean;
  memory: boolean;
  browser: boolean;
  modelPicker: boolean;
  history: boolean;
  nav: boolean;
  monitor: boolean;
}

export type CherryEffortLevel = 'off' | 'minimal' | 'low' | 'medium' | 'high' | 'xhigh';

export interface CherryExportProgress {
  phase:
    | 'waiting-for-conversations'
    | 'collecting'
    | 'redacting'
    | 'packaging'
    | 'transferring'
    | 'complete';
  processedBytes?: number;
  estimatedBytes?: number;
}

export declare class CherryHostTransport extends SyntheticCdpTransport {
  constructor(opts: CherryHostTransportOptions);
  get hostOrigin(): string;
  get joinUrl(): string | null;
  get features(): CherryFeatures;
  get theme(): string | null;
  get layout(): string | null;
  get effortLevel(): CherryEffortLevel | null;
  get flags(): string | null;
  get negotiatedProtocolVersion(): number;
  connect(options?: CDPConnectOptions): Promise<void>;
  disconnect(): void;
  testReceive(event: MessageEvent): void;
  emitSliccEventToHost(name: string, detail?: unknown): void;
  onHostEvent: ((name: string, detail?: unknown) => void) | null;
  onExportRequest:
    | ((
        requestId: string,
        sessionId: string | undefined,
        signal: AbortSignal,
        onProgress: (progress: CherryExportProgress) => void
      ) => Promise<Blob>)
    | null;
}
