export type CdpParams = Record<string, unknown>;

export interface CdpMessage {
  method?: string;
  params?: CdpParams;
  sessionId?: string;
  id?: number;
  result?: unknown;
  error?: { message?: string };
}

export type CdpListener = (message: CdpMessage) => void;

export interface CdpConnectOptions {
  url?: string;
  env?: string;
  runtime?: string;
  timeout?: number;
  fetch?: typeof fetch;
  WebSocket?: typeof WebSocket;
}

export declare function connect(options?: CdpConnectOptions): Promise<CDPClient>;

export declare class CDPClient {
  constructor(socket?: typeof WebSocket);
  open(url: string, options?: { timeout?: number }): Promise<void>;
  send(method: string, params?: CdpParams, sessionId?: string, timeout?: number): Promise<unknown>;
  on(event: string, listener: CdpListener): () => void;
  off(event: string, listener: CdpListener): void;
  attach(targetId: string): Promise<string>;
  close(): void;
}
