export declare const CHERRY_PROTOCOL_VERSION: 2;

export declare const SUPPORTED_CHERRY_PROTOCOL_VERSIONS: readonly number[];

export type CherryCdpPayload = Record<string, unknown>;

export interface CherryHandshakeHello {
  cherry: number;
  channelId: string;
  kind: 'handshake.hello';
  capabilities: { navigate: boolean; screenshot: boolean; openUrl: boolean };
}

export interface CherryHandshakeWelcome {
  cherry: number;
  channelId: string;
  kind: 'handshake.welcome';
  joinUrl?: string;
  features?: {
    terminal: boolean;
    files: boolean;
    memory: boolean;
    browser: boolean;
    modelPicker: boolean;
    history: boolean;
    nav: boolean;
    monitor: boolean;
    showTimestamps: boolean;
  };
  theme?: string;
  layout?: string;
  effortLevel?: 'off' | 'minimal' | 'low' | 'medium' | 'high' | 'xhigh';
  flags?: string;
}

export interface CherryHandshakeVersionMismatch {
  cherry: number;
  channelId: string;
  kind: 'handshake.version-mismatch';
  peerVersion: number;
}

export interface CherryCdpRequest {
  cherry: number;
  channelId: string;
  kind: 'cdp.request';
  id: number;
  method: string;
  params?: CherryCdpPayload;
  sessionId?: string;
}

export interface CherryCdpResponse {
  cherry: number;
  channelId: string;
  kind: 'cdp.response';
  id: number;
  result?: CherryCdpPayload;
  error?: { code: number; message: string };
}

export interface CherryCdpEvent {
  cherry: number;
  channelId: string;
  kind: 'cdp.event';
  method: string;
  params?: CherryCdpPayload;
  sessionId?: string;
}

export interface CherryPermissionRequest {
  cherry: number;
  channelId: string;
  kind: 'permission.request';
  id: number;
  domain: string;
}

export interface CherryPermissionResponse {
  cherry: number;
  channelId: string;
  kind: 'permission.response';
  id: number;
  granted: boolean;
}

export interface CherryHostEvent {
  cherry: number;
  channelId: string;
  kind: 'host.event';
  name: string;
  detail?: unknown;
}

export interface CherrySliccEvent {
  cherry: number;
  channelId: string;
  kind: 'slicc.event';
  name: string;
  detail?: unknown;
}

export interface CherrySessionExportRequest {
  cherry: number;
  channelId: string;
  kind: 'session.export.request';
  requestId: string;
  sessionId?: 'active' | string;
}

export interface CherrySessionExportCancel {
  cherry: number;
  channelId: string;
  kind: 'session.export.cancel';
  requestId: string;
}

export interface CherrySessionExportProgress {
  cherry: number;
  channelId: string;
  kind: 'session.export.progress';
  requestId: string;
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

export interface CherrySessionExportResponse {
  cherry: number;
  channelId: string;
  kind: 'session.export.response';
  requestId: string;
  blob: Blob;
}

export interface CherrySessionExportError {
  cherry: number;
  channelId: string;
  kind: 'session.export.error';
  requestId: string;
  code: string;
}

export type CherryEnvelope =
  | CherryHandshakeHello
  | CherryHandshakeWelcome
  | CherryHandshakeVersionMismatch
  | CherryCdpRequest
  | CherryCdpResponse
  | CherryCdpEvent
  | CherryPermissionRequest
  | CherryPermissionResponse
  | CherryHostEvent
  | CherrySliccEvent
  | CherrySessionExportRequest
  | CherrySessionExportCancel
  | CherrySessionExportProgress
  | CherrySessionExportResponse
  | CherrySessionExportError;

export interface CherryVersionSkew {
  cherry: number;
  channelId: string;
  kind: string;
}

export interface AcceptContext {
  allowOrigins: string[];
  expectedSource: MessageEventSource | null;
  channelId: string | null;
  versions?: readonly number[];
}

export declare function isCherryEnvelope(
  value: unknown,
  versions?: readonly number[]
): value is CherryEnvelope;

export declare function acceptEnvelope(event: MessageEvent, ctx: AcceptContext): boolean;

export declare function isCherryVersionMismatch(
  value: unknown,
  supported?: readonly number[]
): value is CherryVersionSkew;
