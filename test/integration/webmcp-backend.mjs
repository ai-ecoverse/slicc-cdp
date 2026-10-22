import vm from 'node:vm';
import { fakeCdp } from './fake-cdp.mjs';

export async function webmcpBackend({ policy = '', definitions = [] } = {}) {
  let next = 0;
  let page;
  const sessions = new Map();
  const scripts = new Map();
  const permitted = new Map();
  const makePage = (loader) => {
    class Document {}
    class Navigator {}
    const document = new Document();
    const navigator = new Navigator();
    const location = { origin: 'https://ordinary.test' };
    const window = { frames: [], document, location };
    const context = vm.createContext({
      Document,
      Navigator,
      document,
      navigator,
      window,
      location,
      isSecureContext: true,
      EventTarget,
      Event,
      AbortController,
      DOMException,
    });
    return { document, context, loader, window };
  };
  page = makePage('L');
  const frame = () => ({
    id: 'TAB',
    loaderId: page.loader,
    url: 'https://ordinary.test/',
    securityOrigin: 'https://ordinary.test',
  });
  const fake = await fakeCdp({
    intercept(message, send) {
      const { id, method, params = {}, sessionId } = message;
      const answer = (result) => send({ id, result });
      const event = (method, params, peer = sessionId) => send({ method, params, sessionId: peer });
      const created = (peer) =>
        event(
          'Runtime.executionContextCreated',
          {
            context: {
              id: 1,
              uniqueId: `main-${page.loader}`,
              origin: 'https://ordinary.test',
              auxData: { frameId: 'TAB', isDefault: true },
            },
          },
          peer
        );
      const pause = (peer) => {
        event('Debugger.scriptParsed', { scriptId: peer, executionContextId: 1 }, peer);
        event(
          'Debugger.paused',
          {
            callFrames: [
              {
                callFrameId: peer,
                functionName: '__slicc_webmcp_gate',
                location: { scriptId: peer, lineNumber: 0, columnNumber: 0 },
              },
            ],
          },
          peer
        );
      };
      if (method === 'Target.attachToTarget') {
        const peer = `PEER${++next}`;
        sessions.set(peer, params.targetId);
        answer({ sessionId: peer });
        return true;
      }
      if (method === 'Target.detachFromTarget') {
        sessions.delete(params.sessionId);
        scripts.delete(params.sessionId);
        answer({});
        return true;
      }
      if (method === 'Page.getFrameTree') {
        answer({ frameTree: { frame: frame() } });
        return true;
      }
      if (method === 'Browser.getVersion') {
        answer({ product: 'Chrome/156.0.0.0' });
        return true;
      }
      if (method === 'Runtime.enable') {
        created(sessionId);
        answer({});
        return true;
      }
      if (method === 'Page.createIsolatedWorld') {
        event('Runtime.executionContextCreated', {
          context: {
            id: 2,
            uniqueId: `isolated-${page.loader}`,
            origin: 'https://ordinary.test',
            name: 'slicc-webmcp',
            auxData: { frameId: 'TAB', isDefault: false },
          },
        });
        answer({ executionContextId: 2 });
        return true;
      }
      if (method === 'Page.getPermissionsPolicyState') {
        answer({ states: [] });
        return true;
      }
      if (method === 'Page.addScriptToEvaluateOnNewDocument') {
        scripts.set(sessionId, params.source);
        answer({ identifier: sessionId });
        if (params.runImmediately) pause(sessionId);
        return true;
      }
      if (method === 'Page.removeScriptToEvaluateOnNewDocument') {
        scripts.delete(sessionId);
        answer({});
        return true;
      }
      if (method === 'Debugger.getScriptSource') {
        answer({ scriptSource: scripts.get(sessionId) });
        return true;
      }
      if (method === 'Debugger.evaluateOnCallFrame') {
        const value = params.expression.endsWith('true');
        permitted.set(sessionId, value);
        answer({ result: { type: 'boolean', value } });
        return true;
      }
      if (method === 'Debugger.resume') {
        if (permitted.get(sessionId) && scripts.has(sessionId)) {
          vm.runInContext(
            scripts
              .get(sessionId)
              .replace('let __slicc_webmcp_admitted = false', 'let __slicc_webmcp_admitted = true'),
            page.context
          );
          permitted.set(sessionId, false);
          if (page.loader !== 'L' && !page.loaded) {
            page.loaded = true;
            for (const definition of definitions)
              page.document.modelContext?.registerTool(definition);
            for (const peer of sessions.keys())
              event('Page.loadEventFired', { timestamp: 1 }, peer);
          }
        }
        answer({});
        return true;
      }
      if (method === 'Page.reload') {
        page = makePage(`RELOAD${next}`);
        answer({});
        for (const peer of sessions.keys()) {
          event('Page.frameNavigated', { frame: frame() }, peer);
          event('Runtime.executionContextsCleared', {}, peer);
          created(peer);
          event(
            'Network.responseReceived',
            {
              frameId: 'TAB',
              loaderId: page.loader,
              type: 'Document',
              response: {
                url: 'https://ordinary.test/',
                status: 200,
                headers: { 'permissions-policy': policy },
              },
            },
            peer
          );
          if (scripts.has(peer)) pause(peer);
        }
        return true;
      }
      if (method === 'Runtime.evaluate') {
        if (params.expression === 'document.readyState') {
          answer({ result: { type: 'string', value: 'complete' } });
          return true;
        }
        void Promise.resolve().then(async () => {
          try {
            const value = await vm.runInContext(params.expression, page.context);
            answer({ result: { type: typeof value, value } });
          } catch (error) {
            answer({ result: { type: 'undefined' }, exceptionDetails: { text: error.message } });
          }
        });
        return true;
      }
      if (method === 'Page.enable' || method === 'Network.enable' || method === 'Debugger.enable') {
        answer({});
        return true;
      }
      if (
        method === 'Debugger.resume' ||
        method === 'Runtime.disable' ||
        method === 'Target.setAutoAttach'
      ) {
        answer({});
        return true;
      }
      return false;
    },
  });
  fake.targets.push({ targetId: 'TAB', url: 'https://ordinary.test/' });
  const finishLoad = () => {
    if (!page.document.modelContext) return false;
    for (const definition of definitions) page.document.modelContext.registerTool(definition);
    return true;
  };
  return {
    ...fake,
    get page() {
      return page;
    },
    finishLoad,
    sessions,
    scripts,
  };
}
