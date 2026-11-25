import assert from 'node:assert/strict';
import { fakeCdp } from './fake-cdp.mjs';

export const FORM = 'https://example.test/form';
export const SECRET = 'SECRET-DIALOG-TEXT';

export async function dialogFake({ hold: holdCheck, ...options } = {}) {
  const sends = new Map();
  const open = new Set();
  const waiting = new Map();
  const disconnects = [];
  const closes = new Map();
  const fake = await fakeCdp({
    ...options,
    onDisconnect: (send) => disconnects.push(send),
    intercept(message, send, close) {
      const { id, method, sessionId } = message;
      if (sessionId) {
        sends.set(sessionId, send);
        closes.set(sessionId, close);
      }
      if (holdCheck?.(message, send, fake)) return true;
      const target = fake.calls.find((call) => call.sessionId === sessionId)?.target;
      if (method === 'Runtime.evaluate' && target && open.has(target)) {
        const list = waiting.get(target) ?? [];
        list.push(() =>
          send({ id, sessionId, result: { result: { type: 'string', value: 'after-dialog' } } })
        );
        waiting.set(target, list);
        return true;
      }
      if (method === 'Page.handleJavaScriptDialog' && target) {
        open.delete(target);
        send({ id, sessionId, result: {} });
        send({
          method: 'Page.javascriptDialogClosed',
          sessionId,
          params: { result: message.params.accept, userInput: message.params.promptText ?? '' },
        });
        for (const reply of waiting.get(target) ?? []) reply();
        waiting.delete(target);
        return true;
      }
      return false;
    },
  });
  const holdSessions = (targetId) => {
    const enabled = (method) =>
      new Set(
        fake.calls
          .filter((call) => call.method === method && call.target === targetId)
          .map((call) => call.sessionId)
      );
    const runtime = enabled('Runtime.enable');
    return [...enabled('Page.enable')].filter((session) => !runtime.has(session));
  };
  const holdSession = (targetId) => {
    const sessions = holdSessions(targetId);
    assert.ok(sessions.length > 0, `a dialog session on ${targetId}`);
    return sessions.at(-1);
  };
  const detached = () =>
    fake.calls
      .filter((call) => call.method === 'Target.detachFromTarget')
      .map((call) => call.params.sessionId);
  const emit = (sessionId, method, params) => sends.get(sessionId)?.({ method, sessionId, params });
  const openDialog = (targetId, params, sessionId = holdSession(targetId)) => {
    open.add(targetId);
    emit(sessionId, 'Page.javascriptDialogOpening', {
      url: FORM,
      message: SECRET,
      type: 'alert',
      hasBrowserHandler: true,
      defaultPrompt: '',
      frameId: targetId,
      ...params,
    });
  };
  const handled = () => fake.calls.filter((call) => call.method === 'Page.handleJavaScriptDialog');
  return {
    fake,
    sends,
    closes,
    open,
    disconnects,
    holdSession,
    holdSessions,
    detached,
    emit,
    openDialog,
    handled,
  };
}

export async function until(check, label, ms = 5000) {
  const deadline = Date.now() + ms;
  while (!check()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`);
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}
