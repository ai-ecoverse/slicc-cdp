import vm from 'node:vm';
import { fakeCdp } from './fake-cdp.mjs';

export async function evalBackend(options = {}) {
  const emitters = new Map();
  const worlds = new Map();
  const armed = new Set();
  const scripts = new Map();
  const localFrames = (target) => {
    const frames = [];
    const visit = (nodes) => {
      for (const node of nodes ?? []) {
        frames.push(node.frame);
        visit(node.childFrames);
      }
    };
    visit(target?.childFrames ?? target?.tree?.childFrames);
    return frames;
  };
  let backend;
  backend = await fakeCdp({
    ...options,
    onCommand(message, emit) {
      if (message.sessionId)
        emitters.set(message.sessionId, { emit, target: backend.calls.at(-1).target });
      options.onCommand?.(message, emit);
      if (message.method === 'Runtime.enable') {
        const target = backend.targets.find(
          (target) => target.targetId === backend.calls.at(-1).target
        );
        for (const [index, frame] of localFrames(target).entries()) {
          emit('Runtime.executionContextCreated', {
            context: {
              id: 100 + index,
              uniqueId: `default:${frame.id}:${frame.loaderId}`,
              origin: frame.securityOrigin,
              name: '',
              auxData: { frameId: frame.id, isDefault: true },
            },
          });
        }
      }
      if (
        options.resumeBeforeEval &&
        message.method === 'Slicc.action' &&
        message.params.phase === 'start'
      ) {
        for (const session of armed) emitters.get(session).emit('Debugger.resumed', {});
      }
    },
    intercept(message, send, close) {
      if (options.intercept?.(message, send, close)) return true;
      if (message.method === 'Debugger.setInstrumentationBreakpoint') {
        armed.add(message.sessionId);
        send({ id: message.id, result: { breakpointId: 'EARLY' } });
        return true;
      }
      if (message.method === 'Debugger.pause') {
        const scriptId = scripts.get(message.sessionId);
        send({ id: message.id, result: {} });
        emitters.get(message.sessionId).emit('Debugger.paused', {
          reason: 'other',
          callFrames: [{ callFrameId: 'CALL', location: { scriptId } }],
        });
        return true;
      }
      if (message.method === 'Debugger.disable') armed.delete(message.sessionId);
      if (message.method === 'Page.createIsolatedWorld') {
        const target = backend.targets.find(
          (target) => target.targetId === backend.calls.at(-1).target
        );
        const frame = localFrames(target).find((frame) => frame.id === message.params.frameId);
        if (frame) {
          const world = {
            id: 300 + localFrames(target).indexOf(frame),
            uniqueId: `world:${frame.id}:${frame.loaderId}`,
            origin: frame.securityOrigin,
            name: message.params.worldName ?? '',
            auxData: { frameId: frame.id, isDefault: false },
          };
          target.worlds = [
            ...(target.worlds ?? []).filter((known) => known.uniqueId !== world.uniqueId),
            world,
          ];
          emitters
            .get(message.sessionId)
            .emit('Runtime.executionContextCreated', { context: world });
          send({ id: message.id, result: { executionContextId: world.id } });
          return true;
        }
      }
      const { expression, uniqueContextId } = message.params ?? {};
      if (
        message.method !== 'Runtime.evaluate' ||
        !(
          expression?.startsWith('(() => { const queue = [];') ||
          expression?.startsWith('globalThis["__slicc_eval_') ||
          (options.evalCalls && uniqueContextId)
        )
      )
        return false;
      const target = backend.targets.find(
        (target) => target.targetId === backend.calls.at(-1).target
      );
      const frame = localFrames(target).find((frame) => uniqueContextId?.includes(`:${frame.id}:`));
      let context = worlds.get(uniqueContextId);
      if (!context) {
        context = vm.createContext({ location: { href: frame?.url ?? target.url } });
        worlds.set(uniqueContextId, context);
      }
      try {
        const value = vm.runInContext(expression, context, { timeout: 1000 });
        send({
          id: message.id,
          sessionId: message.sessionId,
          result: { result: { type: typeof value, value } },
        });
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
    for (const frame of localFrames(target)) frame.loaderId = `${frame.id}:${loaderId}`;
    target.frame = {
      id: target.targetId,
      url,
      loaderId,
      securityOrigin: new URL(url).origin,
      ...(target.parentFrameId ? { parentId: target.parentFrameId } : {}),
    };
    for (const [session, { emit, target: id }] of emitters) {
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
      for (const [index, frame] of localFrames(target).entries()) {
        emit('Runtime.executionContextCreated', {
          context: {
            id: 100 + index,
            uniqueId: `default:${frame.id}:${frame.loaderId}`,
            origin: frame.securityOrigin,
            name: '',
            auxData: { frameId: frame.id, isDefault: true },
          },
        });
      }
      if (armed.has(session) && !options.noScripts) {
        const scriptId = `script:${loaderId}`;
        scripts.set(session, scriptId);
        if (options.missFirstPause)
          emit('Debugger.scriptParsed', { scriptId: `missed:${loaderId}`, executionContextId: 1 });
        emit('Debugger.scriptParsed', { scriptId, executionContextId: 1 });
        if (!options.missPause)
          emit('Debugger.paused', {
            reason: 'instrumentation',
            data: { scriptId },
            callFrames: [{ callFrameId: 'CALL', location: { scriptId } }],
          });
      }
    }
  };
  return { ...backend, navigate, worlds };
}
