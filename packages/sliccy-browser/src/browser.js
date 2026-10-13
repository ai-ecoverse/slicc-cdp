import { appendFile } from 'node:fs/promises';
import path from 'node:path';
import { connect } from '@ai-ecoverse/slicc-cdp';
import { buildBrowserFetchScript } from './fetch-script.js';
import { routerSource } from './ws-router.js';

const BINDING = '__sliccWsRouterReport';
const LOAD_TIMEOUT_MS = 15000;

let opening;
const sessions = new Map();
const routers = new Set();
const subscribers = new Map();
let seq = 1;
let listening = false;

function openClient() {
  if (!opening) {
    opening = connect().catch((error) => {
      opening = undefined;
      throw error;
    });
  }
  return opening;
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
  const sessionId = await client.attach(targetId);
  await client.send('Page.enable', {}, sessionId).catch(ignoreMethod('Page.enable:'));
  await client.send('Runtime.enable', {}, sessionId).catch(ignoreMethod('Runtime.enable:'));
  await client.send('Network.enable', {}, sessionId).catch(ignoreMethod('Network.enable:'));
  sessions.set(targetId, sessionId);
  listen(client);
  return { client, sessionId };
}

function listen(client) {
  if (listening) return;
  listening = true;
  client.on('Runtime.bindingCalled', (message) => {
    const params = message?.params;
    if (!params || params.name !== BINDING || typeof params.payload !== 'string') return;
    let body;
    try {
      body = JSON.parse(params.payload);
    } catch {
      return;
    }
    if (!body || typeof body.subId !== 'string') return;
    void deliver(body.subId, body.payload);
  });
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

export async function findTab(query = {}) {
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

export async function ensureTab(url, options = {}) {
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

async function evaluate(tab, source, awaitPromise) {
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

async function installRouter(targetId) {
  if (routers.has(targetId)) return;
  const { client, sessionId } = await sessionFor(targetId);
  try {
    await client.send('Runtime.addBinding', { name: BINDING }, sessionId);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (!message.includes('already exists')) throw error;
  }
  await client.send('Page.addScriptToEvaluateOnNewDocument', { source: routerSource }, sessionId);
  const installed = await client.send(
    'Runtime.evaluate',
    { expression: routerSource, returnByValue: true },
    sessionId
  );
  if (installed?.exceptionDetails) {
    throw new Error(exceptionMessage(installed.exceptionDetails));
  }
  routers.add(targetId);
}

async function callRouter(targetId, expression) {
  const { client, sessionId } = await sessionFor(targetId);
  const result = await client.send(
    'Runtime.evaluate',
    { expression, returnByValue: true },
    sessionId
  );
  if (result?.exceptionDetails) {
    throw new Error(exceptionMessage(result.exceptionDetails));
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
      const wire = {};
      if (patch.urlMatch !== undefined) {
        wire.urlMatch =
          patch.urlMatch === null
            ? null
            : patch.urlMatch instanceof RegExp
              ? patch.urlMatch.source
              : patch.urlMatch;
      }
      if (patch.filter !== undefined) wire.filter = patch.filter;
      await callRouter(
        sub.targetId,
        `globalThis.__sliccWsRouter.update(${JSON.stringify(sub.id)}, ${JSON.stringify(wire)})`
      );
      if (wire.urlMatch === null) delete sub.urlMatch;
      else if (wire.urlMatch !== undefined) sub.urlMatch = wire.urlMatch;
      if (patch.filter === null) delete sub.filter;
      else if (patch.filter !== undefined) sub.filter = patch.filter;
      return toInfo(sub);
    },
    async close() {
      if (!subscribers.has(sub.id)) return false;
      subscribers.delete(sub.id);
      await callRouter(
        sub.targetId,
        `globalThis.__sliccWsRouter.unregister(${JSON.stringify(sub.id)})`
      );
      return true;
    },
  };
}

async function deliver(subId, payload) {
  const sub = subscribers.get(subId);
  if (!sub) return;
  try {
    if (sub.forward.sink === 'log') {
      console.log(typeof payload === 'string' ? payload : JSON.stringify(payload));
      return;
    }
    if (sub.forward.sink === 'vfs') {
      const line = `${typeof payload === 'string' ? payload : JSON.stringify(payload)}\n`;
      await appendFile(sub.forward.path, line);
    }
  } catch {
    return;
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
        await installRouter(targetId);
        const id = `wssub-${seq++}-${Date.now().toString(36)}`;
        const sub = {
          id,
          targetId,
          forward: sink,
          createdAt: new Date().toISOString(),
        };
        if (urlMatch !== undefined) sub.urlMatch = urlMatch;
        if (selector !== undefined) sub.filter = selector;
        const registered = { id };
        if (urlMatch !== undefined) registered.urlMatch = urlMatch;
        if (selector !== undefined) registered.filter = selector;
        await callRouter(
          targetId,
          `globalThis.__sliccWsRouter.register(${JSON.stringify(registered)})`
        );
        subscribers.set(id, sub);
        return makeHandle(sub);
      },
    };
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
