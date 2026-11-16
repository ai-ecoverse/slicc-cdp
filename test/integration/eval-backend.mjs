import vm from 'node:vm';
import { fakeCdp } from './fake-cdp.mjs';

export async function evalBackend(options = {}) {
  const emitters = new Map();
  const worlds = new Map();
  let backend;
  backend = await fakeCdp({
    ...options,
    onCommand(message, emit) {
      if (message.sessionId)
        emitters.set(message.sessionId, { emit, target: backend.calls.at(-1).target });
      options.onCommand?.(message, emit);
    },
    intercept(message, send, close) {
      if (options.intercept?.(message, send, close)) return true;
      const { expression, uniqueContextId } = message.params ?? {};
      if (
        message.method !== 'Runtime.evaluate' ||
        !(
          expression?.startsWith('(() => { const queue = [];') ||
          expression?.startsWith('globalThis["__slicc_eval_')
        )
      )
        return false;
      const target = backend.targets.find(
        (target) => target.targetId === backend.calls.at(-1).target
      );
      let context = worlds.get(uniqueContextId);
      if (!context) {
        context = vm.createContext({ location: { href: target.url } });
        worlds.set(uniqueContextId, context);
      }
      try {
        const value = vm.runInContext(expression, context, { timeout: 1000 });
        send({ id: message.id, sessionId: message.sessionId, result: { result: { value } } });
      } catch (error) {
        send({
          id: message.id,
          sessionId: message.sessionId,
          result: { exceptionDetails: { text: error.message } },
        });
      }
      return true;
    },
  });
  const navigate = (target, url, loaderId) => {
    target.url = url;
    target.loaderId = loaderId;
    target.worlds = [];
    target.frame = {
      id: target.targetId,
      url,
      loaderId,
      securityOrigin: new URL(url).origin,
    };
    for (const { emit, target: id } of emitters.values()) {
      if (id !== target.targetId) continue;
      emit('Runtime.executionContextsCleared', {});
      emit('Page.frameNavigated', { frame: target.frame });
      emit('Runtime.executionContextCreated', {
        context: {
          id: 1,
          uniqueId: `default:${target.targetId}:${loaderId}`,
          origin: target.frame.securityOrigin,
          name: '',
          auxData: { frameId: target.targetId, isDefault: true },
        },
      });
    }
  };
  return { ...backend, navigate, worlds };
}
