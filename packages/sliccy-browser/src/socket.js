const GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';
const HEAD_LIMIT = 65536;

export function socketClass(track) {
  return class Socket extends UnrefSocket {
    constructor(url) {
      super(url);
      track(this);
    }
  };
}

class UnrefSocket {
  #listeners = new Map();
  #socket;
  #open = false;
  #closed = false;
  #head = Buffer.alloc(0);
  #frames = Buffer.alloc(0);
  #parts = [];
  #key;

  constructor(url) {
    this.url = String(url);
    this.#start().catch((error) => this.#failed(error));
  }

  addEventListener(type, listener) {
    let set = this.#listeners.get(type);
    if (!set) {
      set = new Set();
      this.#listeners.set(type, set);
    }
    set.add(listener);
  }

  send(text) {
    if (!this.#open || this.#closed) throw new Error('WebSocket is not open');
    this.#write(1, Buffer.from(String(text), 'utf8'));
  }

  close() {
    if (this.#closed) return;
    if (this.#open) {
      const payload = Buffer.alloc(2);
      payload.writeUInt16BE(1000, 0);
      try {
        this.#write(8, payload);
      } catch {}
    }
    this.#finish(1000, '');
    this.#socket?.end();
    this.#socket?.destroy();
  }

  async #start() {
    const target = new URL(this.url);
    const secure = target.protocol === 'wss:';
    if (!secure && target.protocol !== 'ws:')
      throw new Error(`unsupported scheme ${target.protocol}`);
    const host = target.hostname.replace(/^\[(.*)\]$/u, '$1');
    const port = Number(target.port) || (secure ? 443 : 80);
    const [net, crypto, tls] = await Promise.all([
      import('node:net'),
      import('node:crypto'),
      secure ? import('node:tls') : undefined,
    ]);
    if (this.#closed) return;
    this.#key = crypto.randomBytes(16).toString('base64');
    const accept = crypto.createHash('sha1').update(`${this.#key}${GUID}`).digest('base64');
    let socket;
    if (secure) {
      socket = tls.connect({
        host,
        port,
        servername: net.isIP(host) ? undefined : host,
        ALPNProtocols: ['http/1.1'],
      });
    } else {
      socket = net.connect({ host, port });
    }
    this.#socket = socket;
    socket.unref();
    socket.on(secure ? 'secureConnect' : 'connect', () => {
      socket.unref();
      socket.write(
        `GET ${target.pathname}${target.search} HTTP/1.1\r\nHost: ${target.host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: ${this.#key}\r\nSec-WebSocket-Version: 13\r\n\r\n`
      );
    });
    socket.on('data', (chunk) => {
      try {
        if (this.#open) this.#read(chunk);
        else this.#handshake(chunk, accept);
      } catch (error) {
        this.#failed(error);
      }
    });
    socket.on('error', (error) => this.#failed(error));
    socket.on('close', () => {
      if (this.#open) this.#finish(1006, '');
      else this.#failed(new Error('connection closed during the handshake'));
    });
  }

  #handshake(chunk, accept) {
    this.#head = Buffer.concat([this.#head, chunk]);
    const end = this.#head.indexOf('\r\n\r\n');
    if (end < 0) {
      if (this.#head.length > HEAD_LIMIT) throw new Error('handshake response too large');
      return;
    }
    const lines = this.#head.subarray(0, end).toString('latin1').split('\r\n');
    const rest = this.#head.subarray(end + 4);
    this.#head = Buffer.alloc(0);
    if (!/^HTTP\/1\.1 101\b/u.test(lines[0])) {
      throw new Error(`unexpected handshake response: ${lines[0]}`);
    }
    const headers = new Map();
    for (const line of lines.slice(1)) {
      const at = line.indexOf(':');
      if (at > 0) headers.set(line.slice(0, at).trim().toLowerCase(), line.slice(at + 1).trim());
    }
    if (headers.get('sec-websocket-accept') !== accept) {
      throw new Error('handshake response has the wrong Sec-WebSocket-Accept');
    }
    this.#open = true;
    this.#emit('open', {});
    if (rest.length > 0) this.#read(rest);
  }

  #read(chunk) {
    this.#frames = Buffer.concat([this.#frames, chunk]);
    for (;;) {
      const frame = this.#frame();
      if (!frame) return;
      this.#handle(frame);
      if (this.#closed) return;
    }
  }

  #frame() {
    const buffer = this.#frames;
    if (buffer.length < 2) return undefined;
    const second = buffer[1];
    let length = second & 0x7f;
    let offset = 2;
    if (length === 126) {
      if (buffer.length < 4) return undefined;
      length = buffer.readUInt16BE(2);
      offset = 4;
    } else if (length === 127) {
      if (buffer.length < 10) return undefined;
      length = Number(buffer.readBigUInt64BE(2));
      offset = 10;
    }
    const masked = (second & 0x80) !== 0;
    const total = offset + (masked ? 4 : 0) + length;
    if (buffer.length < total) return undefined;
    const payload = Buffer.from(buffer.subarray(total - length, total));
    if (masked) {
      const key = buffer.subarray(offset, offset + 4);
      for (let i = 0; i < payload.length; i += 1) payload[i] ^= key[i % 4];
    }
    this.#frames = buffer.subarray(total);
    return { fin: (buffer[0] & 0x80) !== 0, opcode: buffer[0] & 0x0f, payload };
  }

  #handle({ fin, opcode, payload }) {
    if (opcode === 8) {
      const code = payload.length >= 2 ? payload.readUInt16BE(0) : 1005;
      const reason = payload.length > 2 ? payload.subarray(2).toString('utf8') : '';
      try {
        this.#write(8, payload.subarray(0, 2));
      } catch {}
      this.#finish(code, reason);
      this.#socket?.end();
      return;
    }
    if (opcode === 9) {
      this.#write(10, payload);
      return;
    }
    if (opcode === 10) return;
    if (opcode === 1 || opcode === 2) this.#parts = [{ opcode, payload }];
    else if (opcode === 0 && this.#parts.length > 0) this.#parts.push({ opcode, payload });
    else return;
    if (!fin) return;
    const [{ opcode: kind }] = this.#parts;
    const data = Buffer.concat(this.#parts.map((part) => part.payload));
    this.#parts = [];
    if (kind === 1) this.#emit('message', { data: data.toString('utf8') });
  }

  #write(opcode, payload) {
    const mask = Buffer.alloc(4);
    for (let i = 0; i < 4; i += 1) mask[i] = Math.floor(Math.random() * 256);
    let head;
    if (payload.length < 126) {
      head = Buffer.from([0x80 | opcode, 0x80 | payload.length]);
    } else if (payload.length < 65536) {
      head = Buffer.alloc(4);
      head[0] = 0x80 | opcode;
      head[1] = 0x80 | 126;
      head.writeUInt16BE(payload.length, 2);
    } else {
      head = Buffer.alloc(10);
      head[0] = 0x80 | opcode;
      head[1] = 0x80 | 127;
      head.writeBigUInt64BE(BigInt(payload.length), 2);
    }
    const body = Buffer.from(payload);
    for (let i = 0; i < body.length; i += 1) body[i] ^= mask[i % 4];
    this.#socket.write(Buffer.concat([head, mask, body]));
  }

  #failed(error) {
    if (this.#closed) return;
    if (this.#open) {
      this.#finish(1006, '');
    } else {
      this.#closed = true;
      this.#emit('error', { error });
    }
    this.#socket?.destroy();
  }

  #finish(code, reason) {
    if (this.#closed) return;
    this.#closed = true;
    this.#open = false;
    this.#emit('close', { code, reason });
  }

  #emit(type, event) {
    for (const listener of [...(this.#listeners.get(type) ?? [])]) {
      try {
        listener(event);
      } catch {}
    }
  }
}
