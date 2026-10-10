import {
  acceptEnvelope,
  CHERRY_PROTOCOL_VERSION,
  isCherryEnvelope,
  isCherryVersionMismatch,
  SUPPORTED_CHERRY_PROTOCOL_VERSIONS,
} from './cherry-host-protocol.js';
import { PendingRequestTable } from './pending-request-table.js';
import { SyntheticCdpTransport } from './synthetic-cdp-transport.js';

function report(level, message, details) {
  if (level === 'debug' || level === 'info') return;
  console[level](message, details);
}

const log = {
  debug: report.bind(null, 'debug'),
  info: report.bind(null, 'info'),
  warn: report.bind(null, 'warn'),
  error: report.bind(null, 'error'),
};

const DEFAULT_TIMEOUT = 30000;

const VALID_EXPORT_ERROR_CODES = new Set([
  'permission-denied',
  'redaction-unavailable',
  'session-not-found',
  'transfer-aborted',
  'transfer-corrupt',
  'schema-invalid',
  'attachment-unreadable',
]);

function defaultFeatures() {
  return {
    terminal: true,
    files: true,
    memory: true,
    browser: true,
    modelPicker: true,
    history: true,
    nav: true,
    monitor: true,
  };
}

function exportErrorCodeFromRejection(err) {
  const maybeCode = typeof err === 'object' && err !== null && 'code' in err ? err.code : undefined;
  if (typeof maybeCode === 'string' && VALID_EXPORT_ERROR_CODES.has(maybeCode)) return maybeCode;
  return 'transfer-corrupt';
}

export class CherryHostTransport extends SyntheticCdpTransport {
  constructor(opts) {
    super({
      targetUrl: typeof location !== 'undefined' ? location.href : 'about:blank',
      targetOrigin: opts.targetOrigin,
      title: 'Cherry Host Page',
      ids: {
        target: 'cherry-target',
        session: 'cherry-session',
        frame: 'cherry-frame',
        loader: 'cherry-loader',
      },
    });
    this.opts = opts;
    this.channelId = null;
    this.nextId = 1;
    this.pending = new PendingRequestTable();
    this.connectResolve = null;
    this.connectReject = null;
    this.connectTimer = null;
    this._joinUrl = null;
    this._features = defaultFeatures();
    this._theme = null;
    this._layout = null;
    this._effortLevel = null;
    this._flags = null;
    this.negotiatedVersion = CHERRY_PROTOCOL_VERSION;
    this.boundHandler = (ev) => this.handleMessage(ev);
    this.onHostEvent = null;
    this.onExportRequest = null;
    this.pendingHostExports = new Map();
  }

  get hostOrigin() {
    return this.opts.targetOrigin;
  }

  getCurrentUrl() {
    return typeof location !== 'undefined' ? location.href : super.getCurrentUrl();
  }

  get joinUrl() {
    return this._joinUrl;
  }

  get features() {
    return this._features;
  }

  get theme() {
    return this._theme;
  }

  get layout() {
    return this._layout;
  }

  get effortLevel() {
    return this._effortLevel;
  }

  get flags() {
    return this._flags;
  }

  get negotiatedProtocolVersion() {
    return this.negotiatedVersion;
  }

  async connect(options) {
    if (this._state !== 'disconnected') {
      throw new Error(`Cannot connect: state is ${this._state}`);
    }
    this._state = 'connecting';
    this.negotiatedVersion = CHERRY_PROTOCOL_VERSION;
    this.channelId = `cherry-${crypto.randomUUID()}`;
    if (typeof window !== 'undefined') {
      window.addEventListener('message', this.boundHandler);
    }
    const timeoutMs = options?.timeout ?? DEFAULT_TIMEOUT;
    return new Promise((resolve, reject) => {
      this.connectResolve = resolve;
      this.connectReject = reject;
      this.connectTimer = setTimeout(() => {
        this.connectTimer = null;
        if (typeof window !== 'undefined') {
          window.removeEventListener('message', this.boundHandler);
        }
        this._state = 'disconnected';
        this.channelId = null;
        this.connectResolve = null;
        this.connectReject = null;
        reject(
          new Error(
            `Cherry handshake timed out after ${timeoutMs}ms — no handshake.welcome ` +
              `from the embedding page (host SDK missing, not listening, or ` +
              `version-skewed; check the host page's console)`
          )
        );
      }, timeoutMs);
      for (const version of SUPPORTED_CHERRY_PROTOCOL_VERSIONS) {
        this.post({
          cherry: version,
          channelId: this.channelId,
          kind: 'handshake.hello',
          capabilities: this.opts.capabilities ?? {
            navigate: true,
            screenshot: true,
            openUrl: true,
          },
        });
      }
    });
  }

  failPendingConnect(err) {
    if (this.connectReject === null) return;
    if (this.connectTimer !== null) {
      clearTimeout(this.connectTimer);
      this.connectTimer = null;
    }
    if (typeof window !== 'undefined') {
      window.removeEventListener('message', this.boundHandler);
    }
    this._state = 'disconnected';
    this.channelId = null;
    const reject = this.connectReject;
    this.connectResolve = null;
    this.connectReject = null;
    reject(err);
  }

  disconnect() {
    if (this.connectTimer !== null) {
      clearTimeout(this.connectTimer);
      this.connectTimer = null;
    }
    if (typeof window !== 'undefined') {
      window.removeEventListener('message', this.boundHandler);
    }
    this.pending.rejectAll('Cherry transport disconnected');
    for (const ctrl of this.pendingHostExports.values()) ctrl.abort();
    this.pendingHostExports.clear();
    this._state = 'disconnected';
    this.channelId = null;
  }

  async forward(method, params, _sessionId, timeout = DEFAULT_TIMEOUT) {
    const id = this.nextId++;
    const response = this.pending.issue(
      id,
      timeout,
      `Cherry CDP timed out after ${timeout}ms: ${method}`
    );
    this.post({
      cherry: this.negotiatedVersion,
      channelId: this.channelId,
      kind: 'cdp.request',
      id,
      method,
      params,
    });
    return response;
  }

  testReceive(event) {
    this.handleMessage(event);
  }

  emitSliccEventToHost(name, detail) {
    if (!this.channelId) {
      log.warn('Dropping slicc.event before handshake (no channelId yet)', { name });
      return;
    }
    this.post({
      cherry: this.negotiatedVersion,
      channelId: this.channelId,
      kind: 'slicc.event',
      name,
      detail,
    });
  }

  post(env) {
    this.opts.counterpart.postMessage(env, this.opts.targetOrigin);
  }

  handleMessage(event) {
    const versions =
      this._state === 'connected' ? [this.negotiatedVersion] : SUPPORTED_CHERRY_PROTOCOL_VERSIONS;
    if (
      !acceptEnvelope(event, {
        allowOrigins: this.opts.allowOrigins,
        expectedSource: this.opts.counterpart,
        channelId: this.channelId,
        versions,
      })
    ) {
      this.diagnoseRejectedMessage(event, versions);
      return;
    }
    const env = event.data;
    if (this.negotiatedVersion < 2 && env.kind.startsWith('session.export.')) {
      log.warn('Ignoring a v2-only envelope on a v1-negotiated cherry channel', {
        kind: env.kind,
      });
      return;
    }
    switch (env.kind) {
      case 'handshake.welcome':
        this.handleWelcome(env);
        return;
      case 'cdp.response': {
        if (env.error) {
          this.pending.reject(
            env.id,
            new Error(`Cherry CDP error: ${env.error.message} (${env.error.code})`)
          );
        } else this.pending.resolve(env.id, env.result ?? {});
        return;
      }
      case 'cdp.event':
        this.emit(env.method, {
          ...(env.params ?? {}),
          sessionId: env.sessionId ?? this.syntheticIds.session,
        });
        return;
      case 'host.event':
        this.onHostEvent?.(env.name, env.detail);
        return;
      case 'session.export.request':
        this.handleExportRequest(env);
        return;
      case 'session.export.cancel': {
        const ctrl = this.pendingHostExports.get(env.requestId);
        if (ctrl) {
          ctrl.abort();
          this.pendingHostExports.delete(env.requestId);
        }
        return;
      }
      default:
        return;
    }
  }

  diagnoseRejectedMessage(event, versions) {
    if (isCherryVersionMismatch(event.data, versions)) {
      log.warn('Cherry protocol version mismatch — update the older side', {
        peerVersion: event.data.cherry,
        supportedVersions: [...SUPPORTED_CHERRY_PROTOCOL_VERSIONS],
        origin: event.origin,
      });
      const trustedPeer =
        event.data.channelId === this.channelId &&
        this.opts.allowOrigins.includes(event.origin) &&
        event.source === this.opts.counterpart;
      if (trustedPeer) {
        this.failPendingConnect(
          new Error(
            `Cherry protocol version mismatch (peer v${event.data.cherry}, ` +
              `ours v${CHERRY_PROTOCOL_VERSION}) — update the older side`
          )
        );
      }
      return;
    }
    if (isCherryEnvelope(event.data, SUPPORTED_CHERRY_PROTOCOL_VERSIONS)) {
      log.warn('Rejected a cherry envelope (origin/source/channel mismatch)', {
        origin: event.origin,
        allowOrigins: this.opts.allowOrigins,
      });
    }
  }

  handleWelcome(env) {
    if (this.connectTimer !== null) {
      clearTimeout(this.connectTimer);
      this.connectTimer = null;
    }
    this._state = 'connected';
    this.negotiatedVersion = env.cherry;
    this._joinUrl = env.joinUrl ?? null;
    this._theme = env.theme ?? null;
    this._layout = env.layout ?? null;
    this._effortLevel = env.effortLevel ?? null;
    this._flags = env.flags ?? null;
    this._features = env.features ?? defaultFeatures();
    log.info('Cherry handshake complete', {
      channelId: this.channelId,
      negotiatedVersion: this.negotiatedVersion,
    });
    this.connectResolve?.();
    this.connectResolve = null;
    this.connectReject = null;
  }

  handleExportRequest(env) {
    const { requestId, sessionId } = env;
    if (!this.onExportRequest || !this.channelId) {
      this.postExportError(requestId, 'transfer-aborted');
      return;
    }
    const abort = new AbortController();
    this.pendingHostExports.set(requestId, abort);
    const channelId = this.channelId;
    const onProgress = (progress) => {
      if (!this.pendingHostExports.has(requestId)) return;
      this.post({
        cherry: this.negotiatedVersion,
        channelId,
        kind: 'session.export.progress',
        requestId,
        phase: progress.phase,
        ...(progress.processedBytes !== undefined
          ? { processedBytes: progress.processedBytes }
          : {}),
        ...(progress.estimatedBytes !== undefined
          ? { estimatedBytes: progress.estimatedBytes }
          : {}),
      });
    };
    void this.onExportRequest(requestId, sessionId, abort.signal, onProgress)
      .then((blob) => {
        this.pendingHostExports.delete(requestId);
        this.post({
          cherry: this.negotiatedVersion,
          channelId,
          kind: 'session.export.response',
          requestId,
          blob,
        });
      })
      .catch((err) => {
        this.pendingHostExports.delete(requestId);
        this.postExportError(requestId, exportErrorCodeFromRejection(err));
      });
  }

  postExportError(requestId, code) {
    if (!this.channelId) return;
    this.post({
      cherry: this.negotiatedVersion,
      channelId: this.channelId,
      kind: 'session.export.error',
      requestId,
      code,
    });
  }
}
