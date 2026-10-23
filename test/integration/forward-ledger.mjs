const canonical = (value) => {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (value && typeof value === 'object')
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`)
      .join(',')}}`;
  return JSON.stringify(value);
};

export class ForwardLedger {
  constructor({ raw = false } = {}) {
    this.raw = raw;
    this.ready = raw;
    this.commands = new Map();
    this.listeners = new Set();
    this.closed = false;
  }

  notify() {
    for (const listener of [...this.listeners]) listener();
  }

  request(message) {
    if (this.closed || !Number.isSafeInteger(message.id) || this.commands.has(message.id))
      throw new Error('forward ledger request identity is invalid');
    this.commands.set(message.id, {
      id: message.id,
      method: message.method,
      params: canonical(message.params ?? {}),
      sessionId: message.sessionId,
      sentAt: Date.now(),
      received: this.raw,
      complete: false,
      attempts: new Map(),
      forwarded: new Set(),
      invalid: false,
    });
    this.notify();
  }

  message(message) {
    const audit = message.audit;
    if (!audit) return;
    if (audit.stage === 'ready') {
      if (audit.version !== 1 || this.ready) throw new Error('forward ledger protocol mismatch');
      this.ready = true;
      this.notify();
      return;
    }
    const command = this.commands.get(audit.id);
    if (!command) throw new Error('forward ledger received an unknown command');
    if (audit.stage === 'received') {
      command.invalid ||= command.received || audit.method !== command.method;
      command.received = true;
    } else if (audit.stage === 'attempted') {
      command.invalid ||=
        !command.received ||
        command.complete ||
        !Number.isSafeInteger(audit.upstreamId) ||
        command.attempts.has(audit.upstreamId);
      command.attempts.set(audit.upstreamId, audit.method);
    } else if (audit.stage === 'forwarded') {
      command.invalid ||=
        command.complete || command.attempts.get(audit.upstreamId) !== audit.method;
      command.forwarded.add(audit.upstreamId);
    } else if (audit.stage === 'complete') {
      command.invalid ||= !command.received || command.complete;
      command.complete = true;
    } else command.invalid = true;
    this.notify();
  }

  rawForward(message) {
    const command = this.commands.get(message.id);
    if (!command || command.method !== message.method)
      throw new Error('raw forward ledger identity mismatch');
    command.attempts.set(message.id, message.method);
    command.forwarded.add(message.id);
    this.notify();
  }

  rawReply(message) {
    const command = this.commands.get(message.id);
    if (!command) return;
    command.complete = true;
    this.notify();
  }

  wait(check, ms) {
    if (check() || this.closed) return Promise.resolve();
    return new Promise((resolve) => {
      let timer;
      const listener = () => {
        if (!check() && !this.closed) return;
        clearTimeout(timer);
        this.listeners.delete(listener);
        resolve();
      };
      this.listeners.add(listener);
      timer = setTimeout(() => {
        this.listeners.delete(listener);
        resolve();
      }, ms);
    });
  }

  async initialized(ms) {
    await this.wait(() => this.ready, ms);
    if (!this.ready || this.closed) throw new Error('forward ledger initialization unavailable');
  }

  auditor(sessionId) {
    const bindings = new Map();
    const used = new Set();
    const bind = (descriptor) => {
      const key = `${typeof descriptor.id}:${descriptor.id}`;
      const matches = (command) =>
        command.sessionId === sessionId &&
        command.method === descriptor.method &&
        command.params === canonical(descriptor.params ?? {}) &&
        command.sentAt >= descriptor.sentAt;
      if (bindings.has(key)) {
        const command = this.commands.get(bindings.get(key));
        return command && matches(command) ? command : undefined;
      }
      const command = [...this.commands.values()].find(
        (entry) => !used.has(entry.id) && matches(entry)
      );
      if (command) {
        bindings.set(key, command.id);
        used.add(command.id);
      }
      return command;
    };
    const fact = (descriptor) => {
      const command = bind(descriptor);
      if (!command || !this.ready || this.closed) return { id: descriptor.id, complete: false };
      const attempts = [...command.attempts]
        .filter(([, method]) => method === command.method)
        .map(([id]) => id);
      const forwarded = attempts.some((id) => command.forwarded.has(id));
      const certain = forwarded || attempts.length === 0;
      return {
        id: descriptor.id,
        clientRequestId: command.id,
        complete: command.received && command.complete && !command.invalid && certain,
        ...(certain ? { forwarded } : {}),
      };
    };
    return async ({ historyTrigger, probes = [], final = false }) => {
      const descriptors = [...(historyTrigger ? [historyTrigger] : []), ...probes];
      const ids = descriptors.map((descriptor) => `${typeof descriptor.id}:${descriptor.id}`);
      if (new Set(ids).size !== ids.length)
        return {
          complete: false,
          probes: probes.map((probe) => ({ id: probe.id, complete: false })),
        };
      await this.wait(() => descriptors.every((descriptor) => fact(descriptor).complete), 3000);
      const entries = probes.map(fact);
      return {
        complete: !this.closed && descriptors.every((descriptor) => fact(descriptor).complete),
        probes: entries,
        ...(historyTrigger ? { historyTrigger: fact(historyTrigger) } : {}),
        final,
      };
    };
  }

  historyAudit() {
    const rows = [...this.commands.values()].filter(
      (row) => row.method === 'Page.navigateToHistoryEntry'
    );
    if (!this.ready || rows.length !== 1) return { id: 'history-trigger', complete: false };
    const row = rows[0];
    const attempts = [...row.attempts]
      .filter(([, method]) => method === row.method)
      .map(([id]) => id);
    const forwarded = attempts.some((id) => row.forwarded.has(id));
    const certain = forwarded || attempts.length === 0;
    return {
      id: 'history-trigger',
      clientRequestId: row.id,
      complete: row.received && row.complete && !row.invalid && certain,
      ...(certain ? { forwarded } : {}),
    };
  }

  close() {
    this.closed = true;
    this.notify();
  }
}
