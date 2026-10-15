import { appendFile } from 'node:fs/promises';
import path from 'node:path';
import { connect } from '@ai-ecoverse/slicc-cdp';
import { buildBrowserFetchScript } from './fetch-script.js';

const LOAD_TIMEOUT_MS = 15000;
const HELD_FRAMES = 100;
const RECORDER = `(() => {
  if (globalThis.__sliccWsSent) return true;
  const sent = [];
  const send = WebSocket.prototype.send;
  WebSocket.prototype.send = function (data) {
    if (typeof data === 'string') {
      sent.push([this.url, data]);
      if (sent.length > 64) sent.shift();
    }
    return send.call(this, data);
  };
  const take = (data) => {
    const at = sent.findIndex((entry) => entry[1] === data);
    return at < 0 ? null : sent.splice(at, 1)[0][0];
  };
  Object.defineProperty(globalThis, '__sliccWsSent', { value: Object.freeze({ take }) });
  return true;
})()`;

let opening;
const sessions = new Map();
const tabsBySession = new Map();
const sockets = new Map();
const held = new Map();
const recorders = new Map();
const networkless = new Map();
const subscribers = new Map();
let seq = 1;
let listening = false;
let active = 0;
let idle;

function openClient() {
  if (!opening) {
    opening = connect().catch((error) => {
      opening = undefined;
      throw error;
    });
  }
  return opening;
}

async function using(work) {
  active += 1;
  clearTimeout(idle);
  idle = undefined;
  try {
    return await work();
  } finally {
    active -= 1;
    release();
  }
}

function release() {
  if (active > 0 || subscribers.size > 0 || !opening || idle) return;
  idle = setTimeout(closeIdle, 0);
}

function closeIdle() {
  idle = undefined;
  if (active > 0 || subscribers.size > 0 || !opening) return;
  const closing = opening;
  opening = undefined;
  sessions.clear();
  tabsBySession.clear();
  sockets.clear();
  held.clear();
  recorders.clear();
  networkless.clear();
  listening = false;
  closing.then(
    (client) => client.close(),
    () => undefined
  );
}

function ignoreMethod(prefix) {
  return (error) => {
    const message = error instanceof Error ? error.message : String(error);
    if (message.startsWith(prefix)) return undefined;
    throw error;
  };
}

async function sessionFor(targetId) {
  const client = await openClient();
  const cached = sessions.get(targetId);
  if (cached) return { client, sessionId: cached };
  listen(client);
  const sessionId = await client.attach(targetId);
  tabsBySession.set(sessionId, targetId);
  await client.send('Page.enable', {}, sessionId).catch(ignoreMethod('Page.enable:'));
  await client.send('Runtime.enable', {}, sessionId).catch(ignoreMethod('Runtime.enable:'));
  await client.send('Network.enable', {}, sessionId).catch((error) => {
    ignoreMethod('Network.enable:')(error);
    networkless.set(sessionId, error.message);
  });
  sessions.set(targetId, sessionId);
  return { client, sessionId };
}

function listen(client) {
  if (listening) return;
  listening = true;
  client.on('Network.webSocketCreated', (message) => {
    const { requestId, url } = message?.params ?? {};
    if (typeof requestId !== 'string' || typeof url !== 'string') return;
    sockets.set(socketKey(message.sessionId, requestId), url);
  });
  client.on('Network.webSocketClosed', (message) => {
    const key = socketKey(message?.sessionId, message?.params?.requestId);
    sockets.delete(key);
    held.delete(key);
  });
  client.on('Network.webSocketFrameReceived', (message) => dispatchFrame(message));
  client.on('Network.webSocketFrameSent', (message) => void learnUrl(client, message));
}

function socketKey(sessionId, requestId) {
  return `${sessionId ?? ''} ${requestId ?? ''}`;
}

function dispatchFrame(message) {
  const params = message?.params;
  const response = params?.response;
  if (response?.opcode !== 1 || typeof response.payloadData !== 'string') return;
  const targetId = tabsBySession.get(message.sessionId);
  if (targetId === undefined) return;
  const key = socketKey(message.sessionId, params.requestId);
  const url = sockets.get(key);
  if (url !== undefined) return route(targetId, url, response.payloadData, () => true);
  route(targetId, url, response.payloadData, (sub) => sub.urlMatch === undefined);
  holdFrame(message.sessionId, targetId, key, response.payloadData);
}

function route(targetId, url, raw, which) {
  for (const sub of subscribers.values()) {
    if (sub.targetId !== targetId || !which(sub) || !urlMatches(sub, url)) continue;
    const body = parseFrame(raw, sub.filter?.parseAs);
    if (body === undefined || !whereMatches(sub.filter?.where, body)) continue;
    deliver(sub, project(body, sub.filter?.project));
  }
}

function holdFrame(sessionId, targetId, key, raw) {
  const wanted = [...subscribers.values()].some(
    (sub) => sub.targetId === targetId && sub.urlMatch !== undefined
  );
  if (!wanted) return;
  const frames = held.get(key) ?? [];
  frames.push(raw);
  if (frames.length > HELD_FRAMES) frames.shift();
  held.set(key, frames);
  record(sessionId);
}

function record(sessionId) {
  if (recorders.has(sessionId)) return recorders.get(sessionId);
  const installing = openClient()
    .then((client) =>
      client.send('Runtime.evaluate', { expression: RECORDER, returnByValue: true }, sessionId)
    )
    .catch(() => undefined);
  recorders.set(sessionId, installing);
  return installing;
}

async function learnUrl(client, message) {
  const sessionId = message?.sessionId;
  const { requestId, response } = message?.params ?? {};
  const key = socketKey(sessionId, requestId);
  if (sockets.has(key) || !recorders.has(sessionId)) return;
  if (response?.opcode !== 1 || typeof response.payloadData !== 'string') return;
  await recorders.get(sessionId);
  const found = await client
    .send(
      'Runtime.evaluate',
      {
        expression: `globalThis.__sliccWsSent?.take(${JSON.stringify(response.payloadData)}) ?? null`,
        returnByValue: true,
      },
      sessionId
    )
    .catch(() => undefined);
  const url = found?.result?.value;
  if (typeof url !== 'string' || sockets.has(key)) return;
  sockets.set(key, url);
  const frames = held.get(key) ?? [];
  held.delete(key);
  const targetId = tabsBySession.get(sessionId);
  for (const raw of frames) route(targetId, url, raw, (sub) => sub.urlMatch !== undefined);
}

function urlMatches(sub, url) {
  if (sub.urlMatch === undefined) return true;
  return typeof url === 'string' && sub.urlMatchRe.test(url);
}

function isPlainObject(value) {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function subsetMatch(value, template) {
  for (const key of Object.keys(template)) {
    const expected = template[key];
    const actual = value[key];
    if (isPlainObject(expected)) {
      if (!isPlainObject(actual) || !subsetMatch(actual, expected)) return false;
    } else if (!Object.is(actual, expected)) {
      return false;
    }
  }
  return true;
}

function whereMatches(where, body) {
  if (!isPlainObject(where) || Object.keys(where).length === 0) return true;
  return isPlainObject(body) && subsetMatch(body, where);
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
  if (!Array.isArray(fields) || fields.length === 0 || !isPlainObject(body)) return body;
  const out = {};
  for (const field of fields) if (field in body) out[field] = body[field];
  return out;
}

function targetIdOf(tab) {
  if (typeof tab === 'string') return tab;
  if (tab && typeof tab === 'object' && typeof tab.targetId === 'string') return tab.targetId;
  throw new TypeError('browser: expected a tab handle or targetId string');
}

function patternOf(value) {
  if (value === undefined) return undefined;
  if (value instanceof RegExp) return value.source;
  if (typeof value === 'string') return value;
  throw new TypeError('browser: url match must be a string or RegExp');
}

function compilePattern(source, label) {
  try {
    return new RegExp(source);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    throw new Error(`${label}: invalid url match regex: ${message}`);
  }
}

function hostnameOf(url) {
  try {
    return new URL(url).hostname;
  } catch {
    return null;
  }
}

function originOf(url) {
  try {
    return new URL(url).origin;
  } catch {
    return null;
  }
}

async function listTabs() {
  const client = await openClient();
  const result = await client.send('Target.getTargets');
  const infos = Array.isArray(result?.targetInfos) ? result.targetInfos : [];
  return infos
    .filter((info) => info && info.type === 'page' && typeof info.targetId === 'string')
    .map((info) => ({
      targetId: info.targetId,
      url: typeof info.url === 'string' ? info.url : '',
      title: typeof info.title === 'string' ? info.title : '',
    }));
}

export async function findTab(query) {
  return using(() => findTabNow(query));
}

async function findTabNow(query = {}) {
  const domain = typeof query.domain === 'string' ? query.domain : undefined;
  const urlMatch = patternOf(query.urlMatch);
  if (domain === undefined && urlMatch === undefined) {
    throw new Error('browser.findTab: query requires `domain` or `urlMatch`');
  }
  const tabs = await listTabs();
  if (domain !== undefined) {
    const wanted = domain.toLowerCase();
    return tabs.find((tab) => hostnameOf(tab.url)?.toLowerCase() === wanted) ?? null;
  }
  const re = compilePattern(urlMatch, 'browser.findTab');
  return tabs.find((tab) => re.test(tab.url)) ?? null;
}

export async function ensureTab(url, options) {
  return using(() => ensureTabNow(url, options));
}

async function ensureTabNow(url, options = {}) {
  if (typeof url !== 'string' || url.length === 0) {
    throw new TypeError('browser.ensureTab: url must be a string');
  }
  const tabs = await listTabs();
  const matchUrl = patternOf(options.matchUrl);
  if (matchUrl !== undefined) {
    const re = compilePattern(matchUrl, 'browser.ensureTab');
    const hit = tabs.find((tab) => re.test(tab.url));
    if (hit) return hit;
  } else {
    const wanted = originOf(url);
    if (wanted) {
      const hit = tabs.find((tab) => originOf(tab.url) === wanted);
      if (hit) return hit;
    }
  }
  const client = await openClient();
  const created = await client.send('Target.createTarget', { url: 'about:blank' });
  const targetId = created?.targetId;
  if (typeof targetId !== 'string' || targetId.length === 0) {
    throw new Error('Target.createTarget returned no targetId');
  }
  const { sessionId } = await sessionFor(targetId);
  if (url === 'about:blank') await waitForLoad(client, sessionId);
  else await navigateAndWait(client, sessionId, url);
  const live = (await listTabs()).find((tab) => tab.targetId === targetId);
  return live ?? { targetId, url, title: '' };
}

function evalSource(tab, source) {
  return evaluate(tab, source, false);
}

function evalSourceAsync(tab, source) {
  return evaluate(tab, source, true);
}

export { evalSource as eval, evalSourceAsync as evalAsync };

function evaluate(tab, source, awaitPromise) {
  return using(() => evaluateNow(tab, source, awaitPromise));
}

async function evaluateNow(tab, source, awaitPromise) {
  const { client, sessionId } = await sessionFor(targetIdOf(tab));
  const result = await client.send(
    'Runtime.evaluate',
    {
      expression: serializeEvalSource(source),
      returnByValue: true,
      awaitPromise,
    },
    sessionId
  );
  if (result?.exceptionDetails) {
    throw new Error(exceptionMessage(result.exceptionDetails));
  }
  return unwrapEvalResult(result?.result?.value);
}

export async function cookie(tab, name) {
  if (typeof name !== 'string') throw new TypeError('browser.cookie: name must be a string');
  return using(() => cookieNow(tab, name));
}

async function cookieNow(tab, name) {
  const { client, sessionId } = await sessionFor(targetIdOf(tab));
  const result = await client.send('Network.getCookies', {}, sessionId);
  const cookies = Array.isArray(result?.cookies) ? result.cookies : [];
  const hit = cookies.find((item) => item && item.name === name);
  return hit && typeof hit.value === 'string' ? hit.value : null;
}

export async function localStorage(tab, key) {
  if (typeof key !== 'string') throw new TypeError('browser.localStorage: key must be a string');
  const raw = await evaluate(
    tab,
    `(function(){try{var v=window.localStorage.getItem(${JSON.stringify(key)});return v===null?null:String(v);}catch(e){return null;}})()`,
    false
  );
  if (raw === null || raw === undefined) return null;
  return typeof raw === 'string' ? raw : String(raw);
}

export async function fetch(tab, url, opts = {}) {
  if (typeof url !== 'string') throw new TypeError('browser.fetch: url must be a string');
  const script = await buildBrowserFetchScript(url, opts);
  return evalSourceAsync(tab, script);
}

function serializeEvalSource(source) {
  if (typeof source === 'function') return `(${source.toString()})()`;
  if (typeof source === 'string') return source;
  throw new TypeError('browser.eval/evalAsync: source must be a function or string');
}

function exceptionMessage(details) {
  const description = details.exception?.description;
  if (typeof description === 'string') return description;
  if (typeof details.text === 'string') return details.text;
  return 'evaluation failed';
}

function unwrapEvalResult(value) {
  if (typeof value !== 'string') return value;
  const first = tryParseJson(value);
  if (first === undefined) return value;
  if (first !== null && typeof first === 'object') return first;
  if (typeof first === 'string') {
    const trimmed = first.trim();
    if (trimmed.length > 0 && (trimmed[0] === '{' || trimmed[0] === '[')) {
      const second = tryParseJson(first);
      if (second !== null && typeof second === 'object') return second;
    }
    return first;
  }
  return value;
}

function tryParseJson(text) {
  const trimmed = text.trim();
  if (trimmed.length === 0) return undefined;
  const first = trimmed[0];
  const looksJson =
    first === '{' ||
    first === '[' ||
    first === '"' ||
    first === '-' ||
    (first >= '0' && first <= '9') ||
    trimmed === 'true' ||
    trimmed === 'false' ||
    trimmed === 'null';
  if (!looksJson) return undefined;
  try {
    return JSON.parse(trimmed);
  } catch {
    return undefined;
  }
}

function pageIsSettled(client, sessionId) {
  return client
    .send(
      'Runtime.evaluate',
      { expression: 'document.readyState', returnByValue: true },
      sessionId,
      1000
    )
    .then((result) => {
      const state = result?.result?.value;
      return state !== 'loading' && state !== 'interactive';
    })
    .catch((error) => {
      const message = error instanceof Error ? error.message : String(error);
      if (
        message.startsWith('Runtime.evaluate timed out') ||
        message.startsWith('Runtime.evaluate:')
      ) {
        return true;
      }
      throw error;
    });
}

function navigateAndWait(client, sessionId, url) {
  return new Promise((resolve, reject) => {
    let settled = false;
    let timer;
    const finish = (error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      client.off('Page.loadEventFired', listener);
      if (error) reject(error);
      else resolve();
    };
    const listener = (message) => {
      if (typeof message?.sessionId === 'string' && message.sessionId !== sessionId) return;
      finish();
    };
    client.on('Page.loadEventFired', listener);
    timer = setTimeout(() => finish(), LOAD_TIMEOUT_MS);
    client.send('Page.navigate', { url }, sessionId).then(
      (result) => {
        if (result?.errorText) finish(new Error(`navigate: ${result.errorText}`));
        else if (!result?.loaderId) finish();
      },
      (error) => finish(error)
    );
  });
}

function waitForLoad(client, sessionId) {
  return new Promise((resolve, reject) => {
    let settled = false;
    let timer;
    const finish = (error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      client.off('Page.loadEventFired', listener);
      if (error) reject(error);
      else resolve();
    };
    const listener = (message) => {
      if (typeof message?.sessionId === 'string' && message.sessionId !== sessionId) return;
      finish();
    };
    client.on('Page.loadEventFired', listener);
    timer = setTimeout(() => finish(), LOAD_TIMEOUT_MS);
    pageIsSettled(client, sessionId).then(
      (ready) => {
        if (ready) finish();
      },
      (error) => finish(error)
    );
  });
}

function unsupported(sink) {
  const error = new Error(`browser.websocket: ${sink} is only available inside SLICC`);
  error.code = 'ENOTSUP';
  return error;
}

function validateSink(forward) {
  if (!forward || typeof forward !== 'object' || typeof forward.sink !== 'string') {
    throw new Error('browser.websocket: forward.sink is required');
  }
  switch (forward.sink) {
    case 'webhook':
    case 'scoop':
      throw unsupported(forward.sink);
    case 'vfs':
      if (typeof forward.path !== 'string' || !path.isAbsolute(forward.path)) {
        throw new Error('browser.websocket: vfs sink path must be an absolute path');
      }
      return;
    case 'log':
      return;
    default:
      throw new Error(`browser.websocket: unknown sink "${forward.sink ?? ''}"`);
  }
}

function toInfo(sub) {
  const info = {
    id: sub.id,
    targetId: sub.targetId,
    forward: sub.forward,
    createdAt: sub.createdAt,
  };
  if (sub.urlMatch !== undefined) info.urlMatch = sub.urlMatch;
  if (sub.filter !== undefined) info.filter = sub.filter;
  return info;
}

function makeHandle(sub) {
  return {
    ...toInfo(sub),
    async update(patch = {}) {
      if (patch.urlMatch === null) {
        delete sub.urlMatch;
        delete sub.urlMatchRe;
      } else if (patch.urlMatch !== undefined) {
        const next = patternOf(patch.urlMatch);
        sub.urlMatchRe = compilePattern(next, 'browser.websocket');
        sub.urlMatch = next;
      }
      if (patch.filter === null) delete sub.filter;
      else if (patch.filter !== undefined) sub.filter = patch.filter;
      return toInfo(sub);
    },
    async close() {
      if (!subscribers.has(sub.id)) return false;
      subscribers.delete(sub.id);
      await sub.writing;
      release();
      return true;
    },
  };
}

function deliver(sub, payload) {
  const line = typeof payload === 'string' ? payload : JSON.stringify(payload);
  if (sub.forward.sink === 'log') {
    console.log(line);
    return;
  }
  if (sub.forward.sink === 'vfs') {
    sub.writing = sub.writing
      .then(() => appendFile(sub.forward.path, `${line}\n`))
      .catch(() => undefined);
  }
}

export const websocket = {
  on(tab, opts = {}) {
    const targetId = targetIdOf(tab);
    const urlMatch = patternOf(opts.urlMatch);
    let selector;
    const builder = {
      filter(next) {
        if (typeof next === 'function' || typeof next === 'string') {
          throw new TypeError(
            'browser.websocket: filter must be a declarative JSON object, not a function or string'
          );
        }
        selector = next;
        return builder;
      },
      async forward(sink) {
        validateSink(sink);
        return using(() => subscribe(sink));
      },
    };
    async function subscribe(sink) {
      const urlMatchRe =
        urlMatch === undefined ? undefined : compilePattern(urlMatch, 'browser.websocket');
      const { sessionId } = await sessionFor(targetId);
      if (networkless.has(sessionId)) {
        throw new Error(`browser.websocket: ${networkless.get(sessionId)}`);
      }
      if (urlMatch !== undefined) await record(sessionId);
      const id = `wssub-${seq++}-${Date.now().toString(36)}`;
      const sub = {
        id,
        targetId,
        forward: sink,
        createdAt: new Date().toISOString(),
        writing: Promise.resolve(),
      };
      if (urlMatch !== undefined) {
        sub.urlMatch = urlMatch;
        sub.urlMatchRe = urlMatchRe;
      }
      if (selector !== undefined) sub.filter = selector;
      subscribers.set(id, sub);
      return makeHandle(sub);
    }
    return builder;
  },
  async list() {
    return [...subscribers.values()].map((sub) => toInfo(sub));
  },
};

const browser = {
  findTab,
  ensureTab,
  eval: evalSource,
  evalAsync: evalSourceAsync,
  cookie,
  localStorage,
  fetch,
  websocket,
};

export default browser;
