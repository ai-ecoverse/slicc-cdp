import { createContext, runInContext } from 'node:vm';

export function mousePage() {
  const events = [];
  const listeners = new Map();
  const tasks = [];
  class Event {
    constructor(type, init) {
      Object.assign(this, { isTrusted: false }, init, { type });
    }
  }
  class Element {
    constructor(id, parentNode = null) {
      Object.assign(this, { id, parentNode, isConnected: true });
    }
    dispatchEvent(event) {
      event.target = this;
      events.push(event);
      for (const listener of listeners.get(event.type) ?? []) listener(event);
      return true;
    }
    closest() {
      return null;
    }
    blur() {}
  }
  class ShadowRoot {}
  class Owner extends Element {}
  class Field extends Element {}
  class Channel {
    constructor() {
      this.port1 = { close() {} };
      this.port2 = { postMessage: () => tasks.push(() => this.port1.onmessage()) };
    }
  }
  const pad = new Element('pad');
  const src = new Element('src', pad);
  const dst = new Element('dst', pad);
  const document = {
    activeElement: pad,
    body: pad,
    documentElement: pad,
    visibilityState: 'hidden',
    elementFromPoint: (x) => (x < 200 ? src : dst),
  };
  const context = createContext({
    document,
    window: {},
    Element,
    ShadowRoot,
    HTMLIFrameElement: Owner,
    HTMLFrameElement: Owner,
    HTMLObjectElement: Owner,
    HTMLEmbedElement: Owner,
    HTMLInputElement: Field,
    HTMLTextAreaElement: Field,
    MouseEvent: Event,
    PointerEvent: Event,
    MessageChannel: Channel,
    addEventListener: (type, listener) => {
      if (!listeners.has(type)) listeners.set(type, new Set());
      listeners.get(type).add(listener);
    },
    removeEventListener: (type, listener) => listeners.get(type)?.delete(listener),
  });
  const evaluate = (expression, filename) => {
    const value = runInContext(expression, context, { filename });
    while (tasks.length) tasks.shift()();
    return value;
  };
  const send = (type, params, target = document.elementFromPoint(params.x, params.y)) => {
    target.dispatchEvent(new Event(type, { ...params, isTrusted: true }));
  };
  return { events, evaluate, send, pad, src, dst };
}
