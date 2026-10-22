import { createHash } from 'node:crypto';

const { containsSecret } = await import(
  new URL('./redirect-secret.mjs', import.meta.resolve('@ai-ecoverse/slicc-browsers/package.json'))
    .href
);

export const x1KnownReason = 'slicc-browsers#150';

export function recordX1Client(client) {
  const calls = [];
  const actor = { messages: client.messages, calls, closed: false };
  const send = client.send.bind(client);
  const close = client.close.bind(client);
  client.send = async (method, params = {}, sessionId) => {
    const call = { method, params: structuredClone(params), sessionId };
    calls.push(call);
    try {
      call.result = await send(method, params, sessionId);
      return call.result;
    } catch (error) {
      call.error = error.message;
      throw error;
    }
  };
  client.close = async () => {
    await close();
    actor.closed = true;
  };
  return actor;
}

function protectedRealm(url) {
  const host = url.hostname.toLowerCase().replace(/\.$/, '');
  return host === 'sliccy.ai' || host.endsWith('.sliccy.ai');
}

function scalarClasses(text, tags) {
  const classes = new Set();
  const spans = [];
  const inspect = (candidate) => {
    let url;
    try {
      url = new URL(candidate.startsWith('//') ? `http:${candidate}` : candidate);
    } catch {
      if (/sliccy\.ai/i.test(candidate)) classes.add('protected-url-unparseable');
      return;
    }
    if (!protectedRealm(url)) return;
    if (
      (url.pathname !== '/' && url.pathname !== '') ||
      url.href.includes('?') ||
      url.href.includes('#') ||
      url.username ||
      url.password
    )
      classes.add('protected-url-path-query-fragment');
    else classes.add('protected-origin-only');
  };
  for (const match of text.matchAll(/[a-z][a-z0-9+.-]*:\/\/|(?<![:/])\/\//gi)) {
    const start = match.index;
    const suffix = text.slice(start);
    const end =
      start + (suffix.search(/[\s"'<>)\]}]/) < 0 ? suffix.length : suffix.search(/[\s"'<>)\]}]/));
    inspect(text.slice(start, end));
    spans.push([start, end]);
  }
  for (const match of text.matchAll(
    /\b(?:[a-z0-9-]+\.)*sliccy\.ai(?::\d+)?(?:\/[^\s"'<>)\]}]*|[?#][^\s"'<>)\]}]*)/gi
  )) {
    if (!spans.some(([start, end]) => match.index >= start && match.index < end))
      inspect(`http://${match[0]}`);
  }
  const outside = text
    .split('')
    .map((char, index) =>
      spans.some(([start, end]) => index >= start && index < end) ? ' ' : char
    )
    .join('');
  if (/\/canary\/[^\s"'<>]+/.test(outside)) classes.add('protected-relative-canary-path');
  if (tags.some((tag) => tag && outside.includes(tag))) classes.add('protected-canary-tag');
  return classes;
}

export function x1ValueClasses(value, tags = [], depth = 0, seen = new Set()) {
  const classes = new Set();
  if (value && typeof value === 'object') {
    if (seen.has(value)) return classes;
    seen.add(value);
    for (const field of Object.values(value))
      for (const kind of x1ValueClasses(field, tags, depth, seen)) classes.add(kind);
    return classes;
  }
  if (typeof value !== 'string') return classes;
  for (const kind of scalarClasses(value, tags)) classes.add(kind);
  const key = `${depth}:${value}`;
  if (depth >= 8 || seen.has(key)) return classes;
  seen.add(key);
  const decoded = [];
  try {
    decoded.push(decodeURIComponent(value));
  } catch {}
  try {
    decoded.push(JSON.parse(value));
  } catch {}
  try {
    decoded.push(JSON.parse(`"${value}"`));
  } catch {}
  for (const [chunk] of value.matchAll(/[A-Za-z0-9+/_-]{12,}={0,2}/g))
    for (let offset = 0; offset < 4; offset++)
      decoded.push(Buffer.from(chunk.slice(offset), 'base64').toString('utf8'));
  for (const item of decoded)
    if (item !== value)
      for (const kind of x1ValueClasses(item, tags, depth + 1, seen)) classes.add(kind);
  return classes;
}

export function protectedX1Exposure(value) {
  return [...x1ValueClasses(value)].some((kind) => kind !== 'protected-origin-only');
}

function stringFields(value, path = '$') {
  if (typeof value === 'string') return [{ field: path, value }];
  if (!value || typeof value !== 'object') return [];
  return Object.entries(value).flatMap(([key, item]) => stringFields(item, `${path}.${key}`));
}

const valueHash = (value) => createHash('sha256').update(value).digest('hex');

export function x1Privacy(actors, { ordinary, protectedOrigin }) {
  if (
    !Array.isArray(actors) ||
    actors.length !== 2 ||
    actors.some(
      (actor) =>
        !Array.isArray(actor.messages) ||
        !actor.messages.length ||
        !Array.isArray(actor.calls) ||
        actor.closed !== true
    )
  )
    return { measured: false };
  let origin;
  let marker;
  try {
    origin = new URL(protectedOrigin).origin;
    marker = `canary-marker ${new URL(protectedOrigin).host}`;
    if (new URL(ordinary).origin !== ordinary || origin === ordinary) return { measured: false };
  } catch {
    return { measured: false };
  }
  const echoes = [];
  const exposures = [];
  const diagnostics = [];
  const tags = actors.flatMap((actor) =>
    actor.calls.flatMap((call) => {
      if (call.method !== 'Page.navigate' || typeof call.params?.url !== 'string') return [];
      try {
        const to = new URL(new URL(call.params.url).searchParams.get('to'));
        return protectedRealm(to) && to.pathname.startsWith('/canary/')
          ? [to.pathname.slice('/canary/'.length)]
          : [];
      } catch {
        return [];
      }
    })
  );
  let markerExposed = false;
  let protectedExposed = false;
  let screenshotExposed = false;
  for (const [actorIndex, actor] of actors.entries()) {
    for (const [callIndex, call] of actor.calls.entries()) {
      if (
        call.method === 'Page.captureScreenshot' &&
        typeof call.result?.data === 'string' &&
        call.result.data.length > 0
      ) {
        screenshotExposed = true;
        diagnostics.push({
          actorIndex,
          callIndex,
          event: 'Page.captureScreenshot',
          field: '$.result.data',
          valueClass: 'screenshot-unverified',
          valueHash: valueHash(call.result.data),
        });
      }
    }
    for (const [messageIndex, text] of actor.messages.entries()) {
      let message;
      try {
        message = JSON.parse(text);
      } catch {
        return { measured: false };
      }
      if (!message || typeof message !== 'object' || Array.isArray(message))
        return { measured: false };
      for (const { field, value } of stringFields(message)) {
        if (containsSecret(value, marker))
          diagnostics.push({
            actorIndex,
            messageIndex,
            event: message.method ?? 'response',
            field,
            valueClass: 'protected-canary-marker',
            valueHash: valueHash(value),
          });
      }
      markerExposed ||= containsSecret(message, marker);
      const params = message.params;
      if (
        ['Network.requestWillBeSent', 'Fetch.requestPaused'].includes(message.method) &&
        typeof params?.request?.url === 'string' &&
        typeof params.requestId === 'string' &&
        params.requestId.length > 0 &&
        !containsSecret(params.request.url, marker)
      ) {
        const callIndex = actor.calls.findIndex((call) => {
          if (
            call.method !== 'Page.navigate' ||
            call.sessionId !== message.sessionId ||
            call.params.url !== params.request.url ||
            typeof call.result?.loaderId !== 'string' ||
            !call.result.loaderId
          )
            return false;
          try {
            const outgoing = new URL(call.params.url);
            if (
              outgoing.origin !== ordinary ||
              outgoing.pathname !== '/page' ||
              outgoing.searchParams.get('k') !== 'href' ||
              !outgoing.searchParams.has('tag') ||
              !outgoing.searchParams.has('to')
            )
              return false;
          } catch {
            return false;
          }
          return message.method === 'Network.requestWillBeSent'
            ? params.requestId === call.result.loaderId
            : params.networkId === call.result.loaderId;
        });
        if (callIndex >= 0) {
          const call = actor.calls[callIndex];
          echoes.push({
            actorIndex,
            messageIndex,
            callIndex,
            method: message.method,
            sessionId: message.sessionId,
            requestId: params.requestId,
            networkId: params.networkId,
            loaderId: call.result.loaderId,
            outgoingURL: call.params.url,
            incomingURL: params.request.url,
            byteExact: true,
          });
          delete params.request.url;
        }
      }
      let exposed = false;
      for (const { field, value } of stringFields(message)) {
        for (const valueClass of x1ValueClasses(value, tags)) {
          diagnostics.push({
            actorIndex,
            messageIndex,
            event: message.method ?? 'response',
            field,
            valueClass,
            valueHash: valueHash(value),
          });
          if (valueClass !== 'protected-origin-only') exposed = true;
        }
      }
      if (exposed)
        exposures.push({
          actorIndex,
          messageIndex,
          method: message.method ?? 'response',
          requestId: params?.requestId ?? null,
        });
      protectedExposed ||= exposed;
    }
  }
  const persistentRefusals = actors.every((actor) =>
    ['Page.addScriptToEvaluateOnNewDocument', 'Network.setExtraHTTPHeaders'].every((method) =>
      actor.calls.some(
        (call) =>
          call.method === method &&
          /^slicc: refused: persistent page capabilities are not supported by slicc-cdp/.test(
            call.error ?? ''
          )
      )
    )
  );
  return {
    measured: true,
    markerExposed,
    protectedExposed,
    screenshotExposed,
    persistentRefusals,
    ordinary,
    protectedOrigin: origin,
    echoes,
    exposures,
    diagnostics,
  };
}

export function isExpectedX1Failure150(row) {
  const audit = row.x1Privacy;
  const states = ['control', 'guarded'].map((side) => row.observations?.[side]);
  return (
    row.id === 'X1.after-exit' &&
    ['seven', 'vault'].includes(row.target) &&
    ['prompt', 'lagging'].includes(row.variant) &&
    row.control === 'fail' &&
    row.guarded === 'fail' &&
    row.live === 'pass' &&
    audit?.measured === true &&
    audit.persistentRefusals === true &&
    audit.markerExposed === false &&
    audit.protectedExposed === false &&
    audit.screenshotExposed === false &&
    Array.isArray(audit.echoes) &&
    [0, 1].every((actorIndex) =>
      audit.echoes.some((echo) => echo.actorIndex === actorIndex && echo.byteExact === true)
    ) &&
    states.every(
      (state) =>
        state &&
        (state.instrumented === false || state.headed === false) &&
        state.intercepted === true &&
        state.served === true &&
        state.document === true &&
        state.forged === false &&
        state.scripted === false &&
        state.probed === false &&
        state.paused === false
    ) &&
    Array.isArray(row.errors) &&
    row.errors.length === 2 &&
    row.errors.every(
      (error) =>
        typeof error === 'string' &&
        error.startsWith(`settle: no beacon from ${audit.ordinary} within `) &&
        /^\d+ms$/.test(error.slice(`settle: no beacon from ${audit.ordinary} within `.length))
    )
  );
}

export function x1Disposition(row) {
  return {
    id: row.id,
    target: row.target,
    variant: row.variant,
    reason: x1KnownReason,
    class: 'persistent-capabilities-refused',
    deletionHook: 'fixed X1 vector version for slicc-browsers#150',
    excludedEchoes: row.x1Privacy.echoes,
  };
}
