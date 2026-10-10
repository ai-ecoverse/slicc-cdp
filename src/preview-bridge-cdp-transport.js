import { PendingRequestTable } from './pending-request-table.js';
import { SyntheticCdpTransport } from './synthetic-cdp-transport.js';

const DEFAULT_TIMEOUT = 30000;

const PREVIEW_SYNTHETIC_IDS = {
  target: 'preview-target',
  session: 'preview-session',
  frame: 'preview-frame',
  loader: 'preview-loader',
};

export class PreviewBridgeCdpTransport extends SyntheticCdpTransport {
  constructor(opts) {
    super({
      targetUrl: opts.targetUrl,
      targetOrigin: opts.targetOrigin,
      title: opts.title,
      ids: opts.ids ?? PREVIEW_SYNTHETIC_IDS,
    });
    this.connId = opts.connId;
    this.sendToWorker = opts.send;
    this.nextId = 1;
    this.pending = new PendingRequestTable();
  }

  async connect(_options) {
    this._state = 'connected';
  }

  onCloseTarget() {
    this.sendToWorker({ type: 'bridge.close', connId: this.connId });
  }

  disconnect() {
    this.pending.rejectAll('PreviewBridgeCdpTransport disconnected');
    this._state = 'disconnected';
  }

  async forward(method, params, sessionId, timeout = DEFAULT_TIMEOUT) {
    const id = this.nextId++;
    const response = this.pending.issue(
      id,
      timeout,
      `PreviewBridge CDP timed out after ${timeout}ms: ${method}`
    );
    this.sendToWorker({
      type: 'bridge.cdp.request',
      connId: this.connId,
      id,
      method,
      params,
      sessionId,
    });
    return response;
  }

  deliverResponse(id, payload) {
    if (payload.error) {
      this.pending.reject(
        id,
        new Error(`PreviewBridge CDP error: ${payload.error.message} (${payload.error.code})`)
      );
      return;
    }
    this.pending.resolve(id, payload.result ?? {});
  }
}
