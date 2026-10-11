export function installWsRouter(win) {
  const w = win;
  if (w.__sliccWsRouter) return;

  const subs = new Map();

  function compileUrlMatch(pattern) {
    if (pattern === undefined) return undefined;
    try {
      return new RegExp(pattern);
    } catch {
      return null;
    }
  }

  function isPlainObject(value) {
    return typeof value === 'object' && value !== null && !Array.isArray(value);
  }

  function subsetMatch(value, template) {
    for (const key of Object.keys(template)) {
      const expected = template[key];
      const actual = value[key];
      if (isPlainObject(expected)) {
        if (!isPlainObject(actual)) return false;
        if (!subsetMatch(actual, expected)) return false;
        continue;
      }
      if (!Object.is(actual, expected)) return false;
    }
    return true;
  }

  function parseFrame(raw, parseAs) {
    if (parseAs === 'text') return raw;
    try {
      return JSON.parse(raw);
    } catch {
      return undefined;
    }
  }

  function project(body, fields) {
    if (!fields || fields.length === 0) return body;
    if (!isPlainObject(body)) return body;
    const out = {};
    for (const field of fields) if (field in body) out[field] = body[field];
    return out;
  }

  function report(subId, payload) {
    const reporter = w.__sliccWsRouterReport;
    if (typeof reporter !== 'function') return;
    try {
      reporter(JSON.stringify({ subId, payload }));
    } catch {
      return;
    }
  }

  function urlMatches(sub, url) {
    if (sub.urlMatch === undefined) return true;
    const re = sub._urlMatchRe;
    if (re === null) return false;
    return !re || re.test(url);
  }

  function whereMatches(sub, body) {
    const where = sub.filter?.where;
    if (!where || Object.keys(where).length === 0) return true;
    if (!isPlainObject(body)) return false;
    return subsetMatch(body, where);
  }

  function dispatchFrame(url, raw) {
    if (subs.size === 0) return;
    for (const sub of subs.values()) {
      if (!urlMatches(sub, url)) continue;
      const body = parseFrame(raw, sub.filter?.parseAs);
      if (body === undefined) continue;
      if (!whereMatches(sub, body)) continue;
      report(sub.id, project(body, sub.filter?.project));
    }
  }

  const seen = new WeakSet();
  function wrapInstance(ws) {
    if (seen.has(ws)) return;
    seen.add(ws);
    ws.addEventListener('message', (ev) => {
      if (typeof ev.data !== 'string') return;
      try {
        dispatchFrame(ws.url, ev.data);
      } catch {
        return;
      }
    });
  }

  const WS = w.WebSocket;
  if (!WS) return;
  const origSend = WS.prototype.send;
  WS.prototype.send = function patchedSend(data) {
    wrapInstance(this);
    return origSend.call(this, data);
  };

  function applyUrlMatchPatch(next, patch) {
    if (!Object.prototype.hasOwnProperty.call(patch, 'urlMatch')) return;
    if (patch.urlMatch === null) {
      delete next.urlMatch;
      next._urlMatchRe = undefined;
      return;
    }
    if (typeof patch.urlMatch === 'string') {
      next.urlMatch = patch.urlMatch;
      next._urlMatchRe = compileUrlMatch(patch.urlMatch);
    }
  }

  function applyFilterPatch(next, patch) {
    if (!Object.prototype.hasOwnProperty.call(patch, 'filter')) return;
    if (patch.filter === null) {
      delete next.filter;
      return;
    }
    if (patch.filter !== undefined) next.filter = patch.filter;
  }

  const router = {
    register(sub) {
      subs.set(sub.id, { ...sub, _urlMatchRe: compileUrlMatch(sub.urlMatch) });
    },
    update(id, patch) {
      const cur = subs.get(id);
      if (!cur) return;
      const next = { ...cur, id };
      applyUrlMatchPatch(next, patch);
      applyFilterPatch(next, patch);
      subs.set(id, next);
    },
    unregister(id) {
      subs.delete(id);
    },
  };
  Object.defineProperty(w, '__sliccWsRouter', { value: router, configurable: false });
}

export const routerSource = `(${installWsRouter.toString()})(globalThis);`;
