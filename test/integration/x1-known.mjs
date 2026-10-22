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

export function protectedX1Exposure(value, depth = 0, seen = new Set()) {
  if (value && typeof value === 'object')
    return Object.values(value).some((field) => protectedX1Exposure(field, depth, seen));
  if (typeof value !== 'string') return false;
  if (
    /(?:[a-z][a-z0-9+.-]*:\/\/|\/\/)(?:[a-z0-9-]+\.)*sliccy\.ai\b|(?:[a-z0-9-]+\.)*sliccy\.ai(?::\d+)?\/|\/canary\//i.test(
      value
    )
  )
    return true;
  const key = `${depth}:${value}`;
  if (depth >= 8 || seen.has(key)) return false;
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
  return decoded.some((item) => item !== value && protectedX1Exposure(item, depth + 1, seen));
}

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
  let markerExposed = false;
  let protectedExposed = false;
  let screenshotExposed = false;
  for (const [actorIndex, actor] of actors.entries()) {
    screenshotExposed ||= actor.calls.some(
      (call) =>
        call.method === 'Page.captureScreenshot' &&
        typeof call.result?.data === 'string' &&
        call.result.data.length > 0
    );
    for (const [messageIndex, text] of actor.messages.entries()) {
      let message;
      try {
        message = JSON.parse(text);
      } catch {
        return { measured: false };
      }
      if (!message || typeof message !== 'object' || Array.isArray(message))
        return { measured: false };
      markerExposed ||= containsSecret(message, marker);
      const params = message.params;
      if (
        ['Network.requestWillBeSent', 'Fetch.requestPaused'].includes(message.method) &&
        typeof params?.request?.url === 'string' &&
        typeof params.requestId === 'string' &&
        params.requestId.length > 0
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
            if (new URL(call.params.url).origin !== ordinary) return false;
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
      const exposed = protectedX1Exposure(message);
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
