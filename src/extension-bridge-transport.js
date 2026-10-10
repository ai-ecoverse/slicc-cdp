import { CdpTransportBridge } from './cdp-bridge.js';
import {
  EXTENSION_BRIDGE_PORT_NAME,
  EXTENSION_BRIDGE_PROTOCOL_VERSION,
  isBridgeVersionMismatch,
  isExtensionBridgeEnvelope,
} from './extension-bridge-protocol.js';

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
log.debug('extension-bridge');

const DEFAULT_HANDSHAKE_TIMEOUT = 10000;

function defaultConnect(extensionId, info) {
  const runtime = globalThis.chrome?.runtime;
  if (!runtime?.connect) {
    throw new Error(
      'chrome.runtime.connect is not available in this realm — the page must be in the extension externally_connectable allowlist'
    );
  }
  return runtime.connect(extensionId, info);
}

function buildBridgeOptions(channelId, holder) {
  return {
    label: 'ExtensionBridgeTransport',
    buildCommandEnvelope: (id, method, params, sessionId) => ({
      bridge: EXTENSION_BRIDGE_PROTOCOL_VERSION,
      channelId,
      kind: 'cdp.request',
      id,
      method,
      params,
      sessionId,
    }),
    sendEnvelope: async (envelope) => {
      const port = holder.port;
      if (!port) throw new Error('Extension bridge port not connected');
      port.postMessage(envelope);
    },
    subscribeIncoming: (handler) => {
      const port = holder.port;
      if (!port) throw new Error('Extension bridge subscribeIncoming called without an open port');
      const listener = (msg) => handler(msg);
      port.onMessage.addListener(listener);
      return () => {};
    },
    parseResponse: (envelope) => {
      if (!isExtensionBridgeEnvelope(envelope)) return null;
      if (envelope.channelId !== channelId) return null;
      if (envelope.kind !== 'cdp.response') return null;
      return { id: envelope.id, result: envelope.result, error: envelope.error };
    },
    parseEvent: (envelope) => {
      if (!isExtensionBridgeEnvelope(envelope)) return null;
      if (envelope.channelId !== channelId) return null;
      if (envelope.kind !== 'cdp.event') return null;
      const params = envelope.sessionId
        ? { ...(envelope.params ?? {}), sessionId: envelope.sessionId }
        : envelope.params;
      return { method: envelope.method, params };
    },
    onListenerError: (event, err) => {
      log.warn('Extension bridge listener error', {
        event,
        error: err instanceof Error ? err.message : String(err),
      });
    },
  };
}

export class ExtensionBridgeTransport extends CdpTransportBridge {
  isExtensionBridge = true;

  constructor(opts) {
    const channelId = `bridge-${crypto.randomUUID()}`;
    const portHolder = { port: null };
    super(buildBridgeOptions(channelId, portHolder));
    this.bridgeOpts = opts;
    this.channelId = channelId;
    this.portHolder = portHolder;
    this.resolveWelcome = null;
    this.rejectWelcome = null;
    this.welcomeTimer = null;
    this.intentionalDisconnect = false;
    this.lastJoinUrl = null;
  }

  async connect(options) {
    this.intentionalDisconnect = false;
    const connectFn = this.bridgeOpts.connect ?? defaultConnect;
    const port = connectFn(this.bridgeOpts.extensionId, { name: EXTENSION_BRIDGE_PORT_NAME });
    this.portHolder.port = port;

    const welcomePromise = new Promise((resolve, reject) => {
      this.resolveWelcome = resolve;
      this.rejectWelcome = reject;
    });

    port.onMessage.addListener((raw) => this.handleHandshake(raw));
    port.onDisconnect.addListener(() => this.handlePortDisconnect(port));

    port.postMessage({
      bridge: EXTENSION_BRIDGE_PROTOCOL_VERSION,
      channelId: this.channelId,
      kind: 'handshake.hello',
    });

    const timeoutMs =
      options?.timeout ?? this.bridgeOpts.handshakeTimeoutMs ?? DEFAULT_HANDSHAKE_TIMEOUT;
    this.welcomeTimer = setTimeout(() => {
      if (!this.rejectWelcome) return;
      this.rejectWelcome(new Error(`Extension bridge handshake timed out after ${timeoutMs}ms`));
      this.cleanupHandshake();
      try {
        port.disconnect();
      } catch {}
      this.portHolder.port = null;
    }, timeoutMs);

    await welcomePromise;
    await super.connect(options);
    if (this.lastJoinUrl !== null) {
      this.sendLeaderJoinUrl(this.lastJoinUrl);
    }
  }

  sendLeaderJoinUrl(joinUrl) {
    this.lastJoinUrl = joinUrl;
    const port = this.portHolder.port;
    if (!port) return;
    port.postMessage({
      bridge: EXTENSION_BRIDGE_PROTOCOL_VERSION,
      channelId: this.channelId,
      kind: 'leader.join-url',
      joinUrl,
    });
  }

  disconnect() {
    this.intentionalDisconnect = true;
    this.cleanupHandshake();
    const port = this.portHolder.port;
    if (port) {
      try {
        port.disconnect();
      } catch {}
      this.portHolder.port = null;
    }
    super.disconnect();
  }

  handlePortDisconnect(droppedPort) {
    if (this.portHolder.port !== droppedPort && this.portHolder.port !== null) return;
    this.portHolder.port = null;

    if (this.rejectWelcome) {
      this.rejectWelcome(new Error('Extension bridge port disconnected before welcome'));
      this.cleanupHandshake();
      return;
    }

    if (!this.intentionalDisconnect && this.state === 'connected') {
      super.disconnect();
    }
  }

  testReceive(raw) {
    this.handleHandshake(raw);
  }

  handleHandshake(raw) {
    if (isBridgeVersionMismatch(raw)) {
      log.warn('Extension bridge protocol version mismatch — update the older side', {
        peerVersion: raw.bridge,
        ourVersion: EXTENSION_BRIDGE_PROTOCOL_VERSION,
      });
      if (raw.channelId === this.channelId && this.rejectWelcome) {
        this.rejectWelcome(
          new Error(
            `Extension bridge protocol version mismatch (peer v${raw.bridge}, ` +
              `ours v${EXTENSION_BRIDGE_PROTOCOL_VERSION}) — update the older side`
          )
        );
        this.cleanupHandshake();
      }
      return;
    }
    if (!isExtensionBridgeEnvelope(raw)) return;
    if (raw.channelId !== this.channelId) return;
    if (raw.kind === 'handshake.welcome') {
      this.resolveWelcome?.();
      this.cleanupHandshake();
      return;
    }
    if (raw.kind === 'handshake.rejected') {
      log.warn('Extension bridge handshake rejected', { reason: raw.reason });
      this.rejectWelcome?.(new Error(`Extension bridge handshake rejected: ${raw.reason}`));
      this.cleanupHandshake();
      return;
    }
    if (raw.kind === 'cdp.event' && raw.method === 'Target.detachedFromTarget') {
      this.disconnect();
      return;
    }
    if (raw.kind === 'extension.lick') {
      this.bridgeOpts.onLick?.(raw);
    }
    if (raw.kind === 'extension.discovery') {
      this.bridgeOpts.onDiscovery?.(raw);
    }
    if (raw.kind === 'extension.open-settings') {
      this.bridgeOpts.onOpenSettings?.();
    }
  }

  cleanupHandshake() {
    if (this.welcomeTimer !== null) {
      clearTimeout(this.welcomeTimer);
      this.welcomeTimer = null;
    }
    this.resolveWelcome = null;
    this.rejectWelcome = null;
  }
}
