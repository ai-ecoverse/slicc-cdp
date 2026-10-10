export class CDPClient {
  #WebSocket;
  #ws = null;
  #state = 'disconnected';
  #nextId = 0;
  #pending = new Map();
  #listeners = new Map();
  #failure;
  #localClose = false;

  constructor(WebSocketImpl = globalThis.WebSocket) {
    this.#WebSocket = WebSocketImpl;
  }

  async open(url, options = {}) {
    if (this.#state === 'connecting' || this.#state === 'connected') {
      throw new Error(`Cannot connect: state is ${this.#state}\n`);
    }
    this.#failure = undefined;
    this.#localClose = false;
    this.#state = 'connecting';
    const timeout = options.timeout ?? 10000;
    const WS = this.#WebSocket;
    return new Promise((resolve, reject) => {
      let settled = false;
      let timer;
      const finish = (error) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        if (error) reject(error);
        else resolve();
      };
      let ws;
      try {
        ws = new WS(url);
      } catch (err) {
        this.#state = 'disconnected';
        finish(err instanceof Error ? err : new Error(String(err)));
        return;
      }
      this.#ws = ws;
      timer = setTimeout(() => {
        this.#localClose = true;
        this.#failure = new Error('CDP connection timed out\n');
        this.#state = 'disconnected';
        const socket = this.#ws;
        this.#ws = null;
        closeQuiet(socket);
        finish(this.#failure);
      }, timeout);
      ws.addEventListener('open', () => {
        if (ws !== this.#ws) return;
        this.#state = 'connected';
        finish();
      });
      ws.addEventListener('message', (event) => {
        if (ws !== this.#ws) return;
        this.#onMessage(event?.data);
      });
      ws.addEventListener('error', () => {
        if (ws !== this.#ws) return;
        if (this.#state !== 'connecting') return;
        const error = new Error('CDP connection failed\n');
        this.#failure = error;
        this.#state = 'disconnected';
        finish(error);
      });
      ws.addEventListener('close', (event) => {
        if (ws !== this.#ws) return;
        const error = new Error(closeMessage(event?.code, event?.reason));
        this.#fail(error);
        finish(error);
      });
    });
  }

  send(method, params, sessionId, timeout = 30000) {
    if (this.#failure) return Promise.reject(this.#failure);
    if (this.#state !== 'connected' || !this.#ws) {
      return Promise.reject(new Error('CDP client is not connected\n'));
    }
    const id = ++this.#nextId;
    const message = { id, method };
    if (params !== undefined) message.params = params;
    if (sessionId !== undefined) message.sessionId = sessionId;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pending.delete(id);
        reject(new Error(`${method} timed out\n`));
      }, timeout);
      this.#pending.set(id, { resolve, reject, timer, method });
      try {
        this.#ws.send(JSON.stringify(message));
      } catch (err) {
        clearTimeout(timer);
        this.#pending.delete(id);
        reject(err instanceof Error ? err : new Error(String(err)));
      }
    });
  }

  on(event, listener) {
    let set = this.#listeners.get(event);
    if (!set) {
      set = new Set();
      this.#listeners.set(event, set);
    }
    set.add(listener);
    return () => this.off(event, listener);
  }

  off(event, listener) {
    const set = this.#listeners.get(event);
    if (!set) return;
    set.delete(listener);
    if (set.size === 0) this.#listeners.delete(event);
  }

  async attach(targetId) {
    const result = await this.send('Target.attachToTarget', { targetId, flatten: true });
    const sessionId = result && typeof result.sessionId === 'string' ? result.sessionId : '';
    if (sessionId.length === 0) {
      throw new Error('Target.attachToTarget returned no sessionId\n');
    }
    return sessionId;
  }

  close() {
    if (this.#localClose) return;
    this.#localClose = true;
    this.#fail(new Error('CDP client disconnected\n'));
    const ws = this.#ws;
    this.#ws = null;
    closeQuiet(ws);
  }

  #onMessage(raw) {
    const text = typeof raw === 'string' ? raw : '';
    let message;
    try {
      message = JSON.parse(text);
    } catch (err) {
      this.#fail(jsonError(err));
      return;
    }
    if (this.#settle(message)) return;
    this.#dispatch(message);
  }

  #settle(message) {
    if (!isRecord(message)) return false;
    if (typeof message.id !== 'number') return false;
    const entry = this.#pending.get(message.id);
    if (!entry) return false;
    this.#pending.delete(message.id);
    clearTimeout(entry.timer);
    if (message.error) {
      entry.reject(new Error(`${entry.method}: ${errorText(message.error)}\n`));
    } else {
      entry.resolve(message.result ?? null);
    }
    return true;
  }

  #dispatch(message) {
    if (!isRecord(message)) return;
    if (typeof message.method !== 'string') return;
    const set = this.#listeners.get(message.method);
    if (!set) return;
    for (const listener of set) {
      try {
        listener(message);
      } catch {}
    }
  }

  #fail(error) {
    this.#failure = this.#failure ?? error;
    this.#state = 'closed';
    for (const entry of this.#pending.values()) {
      clearTimeout(entry.timer);
      entry.reject(this.#failure);
    }
    this.#pending.clear();
  }
}

function isRecord(value) {
  return Boolean(value) && typeof value === 'object';
}

function errorText(error) {
  if (isRecord(error) && typeof error.message === 'string') return error.message;
  return 'CDP error';
}

function jsonError(err) {
  const message = err instanceof Error ? err.message : String(err);
  return new Error(`CDP response was not JSON: ${message}\n`);
}

function closeMessage(code, reason) {
  const text = typeof reason === 'string' ? reason : '';
  if (typeof code !== 'number' || code === 1005) return 'websocket closed\n';
  if (text.length === 0) return `websocket closed ${code}\n`;
  return `websocket closed ${code} ${text}\n`;
}

function closeQuiet(ws) {
  if (!ws) return;
  try {
    ws.close();
  } catch {
    return;
  }
}
