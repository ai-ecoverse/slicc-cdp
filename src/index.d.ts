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

export type CftPlatform = 'linux64' | 'linux-arm64' | 'mac-arm64' | 'mac-x64' | 'win32' | 'win64';

export interface FoundBrowser {
  executable: string;
  source: 'env' | 'installed' | 'system';
  version?: string;
}

export interface InstalledBrowser {
  version: string;
  platform: CftPlatform;
  dir: string;
  executable: string;
}

export declare function cftPlatform(platform?: string, arch?: string): CftPlatform | null;
export declare function browsersRoot(
  env?: Record<string, string | undefined>,
  platform?: string,
  home?: string
): string;
export declare function installedBrowsers(options?: {
  root?: string;
  platform?: CftPlatform;
}): Promise<InstalledBrowser[]>;
export declare function findBrowser(options?: {
  env?: Record<string, string | undefined>;
  platform?: string;
  arch?: string;
  root?: string;
  candidates?: string[];
  version?: string;
}): Promise<FoundBrowser | null>;
export declare function installBrowser(options?: {
  channel?: string;
  version?: string;
  root?: string;
  platform?: CftPlatform | null;
  withDeps?: boolean;
  force?: boolean;
  fetch?: typeof fetch;
  log?: (line: string) => void;
}): Promise<InstalledBrowser & { installed: boolean }>;

export interface LaunchedBrowser {
  child: import('node:child_process').ChildProcess;
  executable: string;
  userDataDir: string;
  port: number;
  wsUrl: string;
  close(): Promise<void>;
}

export declare function launchBrowser(options?: {
  executable?: string;
  userDataDir?: string;
  port?: number;
  headless?: boolean;
  sandbox?: boolean;
  args?: string[];
  env?: Record<string, string | undefined>;
  timeout?: number;
}): Promise<LaunchedBrowser>;
