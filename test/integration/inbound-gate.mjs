const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export class InboundGate {
  constructor({ protectedOrigin, ordinaryOrigin, now = Date.now, wait = sleep, seedSource }) {
    this.protectedOrigin = new URL(protectedOrigin).origin;
    this.ordinaryOrigin = new URL(ordinaryOrigin).origin;
    this.now = now;
    this.wait = wait;
    this.seedSource = seedSource;
    this.requests = new Map();
    this.sessions = new Map();
    this.seeds = new Map();
    this.active = null;
    this.closed = false;
  }

  outbound(message) {
    if (message?.id !== undefined)
      this.requests.set(message.id, {
        method: message.method,
        targetId: message.params?.targetId,
        sessionId: message.sessionId,
      });
  }

  record(message) {
    const request = this.requests.get(message?.id);
    if (request) {
      if (request.method === 'Target.attachToTarget' && message.result?.sessionId)
        this.sessions.set(message.result.sessionId, request.targetId);
      this.requests.delete(message.id);
    }
    if (message?.method === 'Target.attachedToTarget')
      this.sessions.set(message.params.sessionId, message.params.targetInfo?.targetId);
    const frame = message?.method === 'Page.frameNavigated' ? message.params?.frame : undefined;
    if (!frame || frame.parentId || !frame.url) return;
    const targetId = this.sessions.get(message.sessionId);
    if (!targetId) return;
    let url;
    try {
      url = new URL(frame.url);
    } catch {
      return;
    }
    if (url.pathname !== '/bfcache') return;
    const tag = url.searchParams.get('n') ?? '';
    if (url.origin === this.protectedOrigin)
      this.seeds.set(targetId, { tag, protectedObserved: true, ordinaryObserved: false });
    else if (url.origin === this.ordinaryOrigin) {
      const seed = this.seeds.get(targetId);
      if (seed && tag === `${seed.tag}-b`) seed.ordinaryObserved = true;
    }
  }

  inbound(message, deliver) {
    if (this.closed) return;
    const active = this.active;
    const ownReply =
      message?.id !== undefined && this.requests.get(message.id)?.sessionId === active?.sessionId;
    const hold =
      active && (active.side === 'guarded' || message?.sessionId === active.sessionId || ownReply);
    if (hold) {
      active.queue.push({ message, deliver });
      if (
        message?.method === 'Page.frameNavigated' &&
        message.params?.type === 'BackForwardCacheRestore' &&
        this.sessions.get(message.sessionId) === active.targetId
      ) {
        active.delayedEvents += 1;
        active.firstDelayedAt ??= this.now();
      }
      return;
    }
    deliver();
    this.record(message);
  }

  observedSeed(targetId) {
    const seed = this.seeds.get(targetId);
    return seed ? Object.freeze({ ...seed }) : undefined;
  }

  arm({ targetId, sessionId, ms, side, gate }) {
    if (this.closed || this.active) throw new Error('inbound gate unavailable');
    if (
      gate !== true ||
      !['control', 'guarded'].includes(side) ||
      !Number.isSafeInteger(ms) ||
      ms < 1500
    )
      throw new Error('inbound gate requires an explicit 1500ms minimum hold');
    if (this.sessions.get(sessionId) !== targetId)
      throw new Error('inbound gate session target mismatch');
    const seed = this.seedSource ? this.seedSource(targetId) : this.observedSeed(targetId);
    const active = {
      targetId,
      sessionId,
      ms,
      side,
      startedAt: this.now(),
      queue: [],
      delayedEvents: 0,
    };
    this.active = active;
    const release = () =>
      (active.releasing ??= (async () => {
        if (this.closed || this.active !== active)
          throw new Error('inbound gate closed before release');
        for (;;) {
          const remaining = (active.firstDelayedAt ?? active.startedAt) + ms - this.now();
          if (remaining <= 0) break;
          await this.wait(remaining);
          if (this.closed) throw new Error('inbound gate closed before release');
        }
        this.active = null;
        const releasedAt = this.now();
        for (const entry of active.queue.splice(0)) {
          entry.deliver();
          this.record(entry.message);
        }
        active.result = {
          delayedEvents: active.delayedEvents,
          firstDelayedAt: active.firstDelayedAt,
          releasedAt,
        };
        return active.result;
      })());
    return {
      targetId,
      sessionId,
      ms,
      gate: true,
      protectedObserved: seed?.protectedObserved === true,
      ordinaryObserved: seed?.ordinaryObserved === true,
      release,
    };
  }

  close() {
    this.closed = true;
    this.active?.queue.splice(0);
    this.active = null;
    this.requests.clear();
    this.sessions.clear();
    this.seeds.clear();
  }
}
