import vm from 'node:vm';
import { fakeCdp } from './fake-cdp.mjs';

export async function webmcpBackend({
  policy = '',
  definitions = [],
  children = [],
  native = false,
  onCommand = () => {},
} = {}) {
  let next = 0;
  let reloads = 0;
  let pages;
  const sessions = new Map();
  const runtimeEnabled = new Set();
  const writers = new Map();
  const scripts = new Map();
  const permitted = new Map();
  const paused = new Map();
  const queues = new Map();
  const lateFrames = new Set();
  let held = null;
  let loadFired = false;
  const configs = [
    { id: 'TAB', origin: 'https://ordinary.test', policy, definitions },
    ...children,
  ];
  const reset = (loader) => {
    pages = new Map(
      configs.map((config, index) => {
        class Document {}
        class Navigator {}
        const document = new Document();
        const navigator = new Navigator();
        const location = { origin: config.origin };
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
        return [
          config.id,
          {
            ...config,
            Document,
            document,
            context,
            loader: `${loader}-${config.id}`,
            window,
            main: index * 2 + 1,
            isolated: index * 2 + 2,
            isolatedCreated: false,
          },
        ];
      })
    );
    pages.get('TAB').window.frames = children.map((child) => pages.get(child.id).window);
    if (native) {
      for (const page of pages.values()) {
        const tools = (page.definitions || []).map((tool) => ({
          ...tool,
          window: page.window,
          origin: page.origin,
        }));
        page.nativeTools = tools;
        Object.defineProperty(page.Document.prototype, 'modelContext', {
          value: {
            getTools: async ({ fromOrigins = [] } = {}) =>
              [...pages.values()]
                .filter(
                  (provider) =>
                    provider.origin === page.origin || fromOrigins.includes(provider.origin)
                )
                .flatMap((provider) => provider.nativeTools),
            executeTool: async (tool, input) =>
              tool.execute(input, { signal: new AbortController().signal }),
          },
        });
      }
    }
  };
  reset('L');
  const frame = (page) => ({
    id: page.id,
    loaderId: page.loader,
    url: `${page.origin}/`,
    securityOrigin: page.origin,
    ...(page.id === 'TAB' ? {} : { parentId: 'TAB' }),
  });
  const tree = () => ({
    frame: frame(pages.get('TAB')),
    childFrames: children.map((child) => ({ frame: frame(pages.get(child.id)) })),
  });
  const fake = await fakeCdp({
    intercept(message, send) {
      const { id, method, params = {}, sessionId } = message;
      if (onCommand(message, pages, send) === true) return true;
      const answer = (result) => send({ id, result });
      const event = (method, params, peer = sessionId) =>
        (writers.get(peer) || send)({ method, params, sessionId: peer });
      const created = (peer) => {
        for (const page of pages.values()) {
          event(
            'Runtime.executionContextCreated',
            {
              context: {
                id: page.main,
                uniqueId: `main-${page.loader}`,
                origin: page.origin,
                auxData: { frameId: page.id, isDefault: true },
              },
            },
            peer
          );
          if (page.isolatedCreated)
            event(
              'Runtime.executionContextCreated',
              {
                context: {
                  id: page.isolated,
                  uniqueId: `isolated-${page.loader}`,
                  origin: page.origin,
                  name: 'slicc-webmcp',
                  auxData: { frameId: page.id, isDefault: false },
                },
              },
              peer
            );
        }
      };
      const pause = (peer) => {
        const frameId = queues.get(peer)?.shift();
        if (!frameId) return false;
        paused.set(peer, frameId);
        const page = pages.get(frameId);
        const scriptId = `${peer}-${frameId}`;
        event('Debugger.scriptParsed', { scriptId, executionContextId: page.main }, peer);
        event(
          'Debugger.paused',
          {
            callFrames: [
              {
                callFrameId: scriptId,
                functionName: '__slicc_webmcp_gate',
                location: { scriptId, lineNumber: 0, columnNumber: 0 },
              },
            ],
          },
          peer
        );
        return true;
      };
      const begin = (peer) => {
        queues.set(peer, [...pages.keys()]);
        pause(peer);
      };
      if (method === 'Target.attachToTarget') {
        const peer = `PEER${++next}`;
        sessions.set(peer, params.targetId);
        writers.set(peer, send);
        answer({ sessionId: peer });
        return true;
      }
      if (method === 'Target.detachFromTarget') {
        sessions.delete(params.sessionId);
        runtimeEnabled.delete(params.sessionId);
        writers.delete(params.sessionId);
        scripts.delete(params.sessionId);
        answer({});
        if (params.sessionId === 'LATE_MARKER') {
          const release = held;
          held = null;
          release?.();
        }
        return true;
      }
      if (method === 'Page.getFrameTree') {
        answer({ frameTree: tree() });
        return true;
      }
      if (method === 'Browser.getVersion') {
        answer({ product: 'Chrome/156.0.0.0' });
        return true;
      }
      if (method === 'Runtime.enable') {
        runtimeEnabled.add(sessionId);
        created(sessionId);
        answer({});
        return true;
      }
      if (method === 'Runtime.disable') {
        runtimeEnabled.delete(sessionId);
        answer({});
        return true;
      }
      if (method === 'Page.createIsolatedWorld') {
        const page = pages.get(params.frameId);
        page.isolatedCreated = true;
        for (const peer of runtimeEnabled)
          event(
            'Runtime.executionContextCreated',
            {
              context: {
                id: page.isolated,
                uniqueId: `isolated-${page.loader}`,
                origin: page.origin,
                name: 'slicc-webmcp',
                auxData: { frameId: page.id, isDefault: false },
              },
            },
            peer
          );
        answer({ executionContextId: page.isolated });
        return true;
      }
      if (method === 'Page.getPermissionsPolicyState') {
        const page = pages.get(params.frameId);
        answer({
          states: native ? [{ feature: 'tools', allowed: page?.permitted !== false }] : [],
        });
        return true;
      }
      if (method === 'DOM.getDocument') {
        answer({ root: { nodeId: 100 } });
        return true;
      }
      if (method === 'DOM.querySelectorAll') {
        answer({ nodeIds: children.map((_, index) => index + 1) });
        return true;
      }
      if (method === 'DOM.describeNode') {
        const child = children[params.nodeId - 1];
        answer({ node: { backendNodeId: params.nodeId, frameId: child.id } });
        return true;
      }
      if (method === 'DOM.getFrameOwner') {
        answer({ backendNodeId: children.findIndex((child) => child.id === params.frameId) + 1 });
        return true;
      }
      if (method === 'DOM.resolveNode') {
        answer({ object: { type: 'object', objectId: `owner-${params.backendNodeId}` } });
        return true;
      }
      if (method === 'Runtime.callFunctionOn') {
        const index = Number(params.objectId.slice(6)) - 1;
        const child = children[index];
        answer({
          result: {
            type: 'object',
            value: { index, allow: child.allow || '', srcOrigin: child.origin },
          },
        });
        return true;
      }
      if (method === 'Page.addScriptToEvaluateOnNewDocument') {
        scripts.set(sessionId, params.source);
        answer({ identifier: sessionId });
        if (params.runImmediately) begin(sessionId);
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
      const loadEverywhere = () => {
        loadFired = true;
        for (const peer of sessions.keys()) event('Page.loadEventFired', { timestamp: 1 }, peer);
      };
      if (method === 'Debugger.resume') {
        const page = pages.get(paused.get(sessionId));
        if (page && permitted.get(sessionId) && scripts.has(sessionId)) {
          vm.runInContext(
            scripts
              .get(sessionId)
              .replace('let __slicc_webmcp_admitted = false', 'let __slicc_webmcp_admitted = true'),
            page.context
          );
          permitted.set(sessionId, false);
          if (!page.loader.startsWith('L-') && !page.loaded) {
            page.loaded = true;
            for (const definition of page.definitions || [])
              page.document.modelContext?.registerTool?.(definition);
          }
        }
        answer({});
        if (page?.loaded && lateFrames.delete(queues.get(sessionId)?.[0])) {
          held = () => pause(sessionId);
          loadEverywhere();
          event('Target.attachedToTarget', {
            sessionId: 'LATE_MARKER',
            targetInfo: {
              targetId: 'LATE_MARKER',
              type: 'iframe',
              url: 'https://seven.sliccy.ai/',
            },
            waitingForDebugger: true,
          });
          return true;
        }
        if (!pause(sessionId) && page?.loaded && !loadFired) loadEverywhere();
        return true;
      }
      if (method === 'Page.reload') {
        reset(`RELOAD${++reloads}`);
        loadFired = false;
        answer({});
        for (const peer of sessions.keys()) {
          for (const page of pages.values())
            event('Page.frameNavigated', { frame: frame(page) }, peer);
          event('Runtime.executionContextsCleared', {}, peer);
          created(peer);
          for (const page of pages.values())
            event(
              'Network.responseReceived',
              {
                frameId: page.id,
                loaderId: page.loader,
                type: 'Document',
                response: {
                  url: `${page.origin}/`,
                  status: 200,
                  headers: { 'permissions-policy': page.policy || '' },
                },
              },
              peer
            );
          if (scripts.has(peer)) begin(peer);
        }
        return true;
      }
      if (method === 'Runtime.evaluate') {
        const page =
          [...pages.values()].find(
            (page) =>
              params.uniqueContextId === `main-${page.loader}` ||
              params.uniqueContextId === `isolated-${page.loader}` ||
              params.contextId === page.main ||
              params.contextId === page.isolated
          ) || pages.get('TAB');
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
      if (
        [
          'Page.enable',
          'Network.enable',
          'Debugger.enable',
          'Runtime.disable',
          'Target.setAutoAttach',
        ].includes(method)
      ) {
        answer({});
        return true;
      }
      return false;
    },
  });
  fake.targets.push({ targetId: 'TAB', url: 'https://ordinary.test/' });
  return {
    ...fake,
    get page() {
      return pages.get('TAB');
    },
    get pages() {
      return pages;
    },
    sessions,
    scripts,
    admitLate(frameId) {
      lateFrames.add(frameId);
    },
  };
}
