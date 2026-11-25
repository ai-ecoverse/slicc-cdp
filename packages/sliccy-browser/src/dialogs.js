import { connect } from './connect.js';
import { isProtectedUrl } from './guard.js';
import { socketClass } from './socket.js';

const ENABLE_TIMEOUT_MS = 10000;
const CHECK_TIMEOUT_MS = 5000;
const KEPT = 50;

let link;
let lastUrl;
const holds = new Map();
const handlers = new Map();
const records = new Map();
const unreported = new Map();
const frozen = new Map();

export function ordinaryUrl(text) {
  if (typeof text !== 'string' || isProtectedUrl(text) || !URL.canParse(text)) return false;
  const url = new URL(text);
  return (url.protocol === 'http:' || url.protocol === 'https:') && url.hostname.length > 0;
}

function text(value) {
  return typeof value === 'string' ? value : '';
}

function openLink(url) {
  if (link?.url === url) return link.ready;
  closeLink();
  if (lastUrl !== url) {
    lastUrl = url;
    handlers.clear();
    records.clear();
    unreported.clear();
    frozen.clear();
  }
  const current = { url, sessions: new Map() };
  const Socket = socketClass((socket) => {
    current.socket = socket;
    socket.addEventListener('close', () => dropLink(current));
    socket.addEventListener('error', () => dropLink(current));
  });
  current.ready = connect({ url, WebSocket: Socket }).then((client) => {
    current.client = client;
    listen(current, client);
    if (link !== current) client.close();
    return client;
  });
  current.ready.catch(() => dropLink(current));
  link = current;
  return current.ready;
}

function dropLink(current) {
  if (link !== current) return;
  link = undefined;
  for (const [targetId, hold] of holds) {
    if (hold.link !== current) continue;
    holds.delete(targetId);
    release(hold);
  }
  current.client?.close();
  current.socket?.close();
}

function closeLink() {
  if (link) dropLink(link);
}

function listen(current, client) {
  const on = (method, work) =>
    client.on(method, (message) => {
      if (link !== current) return;
      const hold = current.sessions.get(message?.sessionId);
      if (hold && holds.get(hold.targetId) === hold) work(hold, message.params ?? {});
    });
  on('Page.javascriptDialogOpening', (hold, params) => void opened(hold, params));
  on('Page.javascriptDialogClosed', (hold) => forget(hold));
  on('Page.frameNavigated', (hold, params) => navigated(hold, params.frame));
  on('Page.frameStartedNavigating', (hold) => moved(hold));
  on('Inspector.detached', (hold) => drop(hold));
  client.on('Target.detachedFromTarget', (message) => {
    if (link !== current) return;
    const hold = current.sessions.get(message?.params?.sessionId);
    if (hold) drop(hold);
  });
}

function forget(hold) {
  hold.epoch += 1;
  hold.dialog = undefined;
}

function moved(hold) {
  hold.moves += 1;
  forget(hold);
}

function navigated(hold, frame) {
  moved(hold);
  if (frame?.id !== hold.targetId || frame.parentId !== undefined) return;
  if (!ordinaryUrl(`${text(frame.url)}${text(frame.urlFragment)}`)) drop(hold);
}

function release(hold) {
  forget(hold);
  hold.link.sessions.delete(hold.sessionId);
}

function drop(hold) {
  if (holds.get(hold.targetId) === hold) holds.delete(hold.targetId);
  release(hold);
  if (hold.sessionId && hold.link.client && link === hold.link) {
    hold.link.client
      .send('Target.detachFromTarget', { sessionId: hold.sessionId }, undefined, CHECK_TIMEOUT_MS)
      .catch(() => undefined);
  }
  if (link === hold.link && ![...holds.values()].some((other) => other.link === link)) closeLink();
}

export async function hold(url, targetId) {
  const existing = holds.get(targetId);
  if (existing && existing.link === link && link?.url === url) return existing.ready;
  let client;
  try {
    client = await openLink(url);
  } catch {
    return false;
  }
  const again = holds.get(targetId);
  if (again && again.link === link) return again.ready;
  const entry = { targetId, link, epoch: 0, moves: 0, dialog: undefined };
  holds.set(targetId, entry);
  entry.ready = (async () => {
    const sessionId = await client.attach(targetId);
    entry.sessionId = sessionId;
    entry.link.sessions.set(sessionId, entry);
    await client.send('Page.enable', {}, sessionId, ENABLE_TIMEOUT_MS);
    if (holds.get(targetId) !== entry) return false;
    frozen.delete(targetId);
    return true;
  })().catch(() => {
    drop(entry);
    return false;
  });
  return entry.ready;
}

export function setHandler(targetId, handler) {
  handlers.set(targetId, handler);
}

export function clearHandler(targetId, handler) {
  if (handler !== undefined && handlers.get(targetId) !== handler) return false;
  return handlers.delete(targetId);
}

function opened(hold, params) {
  moved(hold);
  const dialog = {
    epoch: hold.epoch,
    frameId: text(params.frameId),
    url: text(params.url),
    type: text(params.type),
    message: text(params.message),
    defaultPrompt: text(params.defaultPrompt),
    state: 'pending',
    done: Promise.withResolvers(),
  };
  hold.dialog = dialog;
  return settle(hold, dialog);
}

function current(hold, dialog) {
  return holds.get(hold.targetId) === hold && hold.dialog === dialog && hold.epoch === dialog.epoch;
}

async function owned(hold, dialog) {
  if (!current(hold, dialog)) return 'stale';
  if (dialog.frameId !== hold.targetId || !ordinaryUrl(dialog.url)) return 'foreign';
  const client = hold.link.client;
  const info = await client
    .send('Target.getTargetInfo', { targetId: hold.targetId }, undefined, CHECK_TIMEOUT_MS)
    .catch(() => undefined);
  const target = info?.targetInfo;
  if (target?.targetId !== hold.targetId || target.type !== 'page') return 'protected';
  if (!ordinaryUrl(target.url) || target.url !== dialog.url) return 'protected';
  if (!current(hold, dialog)) return 'stale';
  const history = await client
    .send('Page.getNavigationHistory', {}, hold.sessionId, CHECK_TIMEOUT_MS)
    .catch(() => undefined);
  const entries = Array.isArray(history?.entries) ? history.entries : [];
  const entry = Number.isInteger(history?.currentIndex) ? entries[history.currentIndex] : undefined;
  if (!ordinaryUrl(entry?.url) || entry.url !== dialog.url) return 'protected';
  return current(hold, dialog) ? 'owned' : 'stale';
}

function blank(dialog) {
  dialog.url = '';
  dialog.type = '';
  dialog.message = '';
  dialog.defaultPrompt = '';
}

async function refuse(hold, dialog, outcome) {
  blank(dialog);
  dialog.done.resolve(false);
  if (outcome === 'stale' || !current(hold, dialog)) return false;
  dialog.state = 'refused';
  remember(hold.targetId, { targetId: hold.targetId, refused: true, at: Date.now() });
  note(hold.targetId, refusal(hold.targetId));
  if (outcome !== 'protected') return false;
  const info = await hold.link.client
    .send('Target.getTargetInfo', { targetId: hold.targetId }, undefined, CHECK_TIMEOUT_MS)
    .catch(() => undefined);
  frozen.set(hold.targetId, text(info?.targetInfo?.url));
  drop(hold);
  return false;
}

export function refusal(targetId) {
  return `browser: tab ${targetId} is showing a dialog this script may not answer, because its page isn't a verified ordinary page. The page waits until the dialog is answered in the browser, or the tab is reloaded or closed.`;
}

function fallback(type) {
  return { accept: type === 'alert' || type === 'beforeunload', by: 'default' };
}

async function choose(hold, dialog) {
  const handler = handlers.get(hold.targetId);
  if (typeof handler !== 'function') return fallback(dialog.type);
  let choice;
  const chosen = Promise.withResolvers();
  const view = Object.freeze({
    targetId: hold.targetId,
    type: dialog.type,
    message: dialog.message,
    defaultPrompt: dialog.defaultPrompt,
    url: dialog.url,
    accept(promptText) {
      if (promptText !== undefined && typeof promptText !== 'string') {
        throw new TypeError('browser.onDialog: promptText must be a string');
      }
      choice ??= { accept: true, promptText, by: 'handler' };
      chosen.resolve();
      return dialog.done.promise;
    },
    dismiss() {
      choice ??= { accept: false, by: 'handler' };
      chosen.resolve();
      return dialog.done.promise;
    },
  });
  const ran = Promise.resolve()
    .then(() => handler(view))
    .catch((error) => {
      const message = error instanceof Error ? error.message : String(error);
      console.warn(`browser.onDialog: the handler for tab ${hold.targetId} failed: ${message}`);
    });
  await Promise.race([chosen.promise, ran]);
  return choice ?? fallback(dialog.type);
}

async function settle(hold, dialog) {
  const before = await owned(hold, dialog);
  if (before !== 'owned') return refuse(hold, dialog, before);
  const choice = await choose(hold, dialog);
  const after = await owned(hold, dialog);
  if (after !== 'owned') return refuse(hold, dialog, after);
  const params = { accept: choice.accept };
  if (choice.accept && typeof choice.promptText === 'string') params.promptText = choice.promptText;
  const moves = hold.moves;
  try {
    await hold.link.client.send(
      'Page.handleJavaScriptDialog',
      params,
      hold.sessionId,
      CHECK_TIMEOUT_MS
    );
  } catch {
    if (current(hold, dialog)) forget(hold);
    dialog.done.resolve(false);
    return false;
  }
  if (hold.moves !== moves) {
    blank(dialog);
    remember(hold.targetId, {
      targetId: hold.targetId,
      accepted: choice.accept,
      by: choice.by,
      unsure: true,
      at: Date.now(),
    });
    note(hold.targetId, unsure(hold.targetId, choice.accept));
    dialog.done.resolve(true);
    return true;
  }
  const record = {
    targetId: hold.targetId,
    type: dialog.type,
    message: dialog.message,
    url: dialog.url,
    accepted: choice.accept,
    by: choice.by,
    at: Date.now(),
  };
  if (dialog.type === 'prompt') record.defaultPrompt = dialog.defaultPrompt;
  if (params.promptText !== undefined) record.promptText = params.promptText;
  remember(hold.targetId, record);
  if (choice.by === 'default') {
    note(
      hold.targetId,
      `browser: tab ${hold.targetId} showed ${article(dialog.type)} ${dialog.type} dialog nobody answered, so it was ${choice.accept ? 'accepted' : 'dismissed'}: ${JSON.stringify(dialog.message)}`
    );
  }
  if (current(hold, dialog)) {
    dialog.state = 'answered';
    hold.dialog = undefined;
  }
  dialog.done.resolve(true);
  return true;
}

function unsure(targetId, accepted) {
  return `browser: tab ${targetId} navigated or opened another dialog while a dialog was being ${accepted ? 'accepted' : 'dismissed'}, so which dialog the answer reached isn't known.`;
}

function article(type) {
  return /^[aeiou]/u.test(type) ? 'an' : 'a';
}

function remember(targetId, record) {
  const list = records.get(targetId) ?? [];
  list.push(Object.freeze(record));
  if (list.length > KEPT) list.shift();
  records.set(targetId, list);
}

function note(targetId, line) {
  const list = unreported.get(targetId) ?? [];
  list.push(line);
  if (list.length > KEPT) list.shift();
  unreported.set(targetId, list);
}

export function report(targetId) {
  const lines = unreported.get(targetId);
  if (!lines) return;
  unreported.delete(targetId);
  for (const line of lines) console.warn(line);
}

export async function blocked(targetId, urlOf) {
  if (holds.get(targetId)?.dialog?.state === 'refused') return true;
  if (!frozen.has(targetId)) return false;
  if ((await urlOf()) === frozen.get(targetId)) return true;
  frozen.delete(targetId);
  return false;
}

export function dialogs(targetId) {
  if (targetId === undefined) return [...records.values()].flat();
  return [...(records.get(targetId) ?? [])];
}
