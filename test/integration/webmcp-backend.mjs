import vm from 'node:vm';
import { fakeCdp } from './fake-cdp.mjs';

export async function webmcpBackend({
  policy = '',
  definitions = [],
  children = [],
  native = false,
  reentrantPauses = false,
  attachDuringRearm = false,
  installerWaitsForResume = false,
  onCommand = () => {},
  beforeNavigateFrame = () => {},
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
  const pendingInstallers = new Map();
  const lateFrames = new Set();
  const autoAttached = new Set();
  const autoParents = new Set();
  let held = null;
  let navigateLatest = () => {};
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
                .flatMap((provider) => provider.nativeTools)
                .filter(
                  (tool) =>
                    tool.origin === page.origin ||
                    !tool.exposedTo ||
                    tool.exposedTo.includes(page.origin)
                ),
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
    url: page.blank ? 'about:blank' : `${page.origin}/`,
    securityOrigin: page.origin,
    ...(page.id === 'TAB' ? {} : { parentId: 'TAB' }),
  });
  const tree = () => ({
    frame: frame(pages.get('TAB')),
    childFrames: children
      .filter((child) => !child.oopif)
      .map((child) => ({ frame: frame(pages.get(child.id)) })),
  });
  const fake = await fakeCdp({
    intercept(message, send) {
      const { id, method, params = {}, sessionId } = message;
      if (onCommand(message, pages, send, sessions) === true) return true;
      const answer = (result) => send({ id, result });
      const event = (method, params, peer = sessionId) =>
        (writers.get(peer) || send)({ method, params, sessionId: peer });
      const localPages = (peer) => {
        const target = sessions.get(peer) || 'TAB';
        return target === 'TAB'
          ? [...pages.values()].filter((page) => !page.oopif)
          : [pages.get(target)].filter(Boolean);
      };
      const attachOopifs = (parent) => {
        for (const page of pages.values()) {
          if (!page.oopif || autoAttached.has(`${parent}:${page.id}`)) continue;
          autoAttached.add(`${parent}:${page.id}`);
          const peer = `PEER${++next}`;
          sessions.set(peer, page.id);
          writers.set(peer, send);
          event(
            'Target.attachedToTarget',
            {
              sessionId: peer,
              targetInfo: {
                targetId: page.id,
                type: 'iframe',
                parentFrameId: 'TAB',
                url: `${page.origin}/`,
              },
              waitingForDebugger: true,
            },
            parent
          );
        }
      };
      const created = (peer) => {
        for (const page of localPages(peer)) {
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
                functionName: '',
                location: { scriptId, lineNumber: 0, columnNumber: 0 },
              },
            ],
          },
          peer
        );
        return true;
      };
      const begin = (peer) => {
        queues.set(
          peer,
          localPages(peer)
            .filter((page) => page.id === (sessions.get(peer) || 'TAB'))
            .map((page) => page.id)
        );
        pause(peer);
      };
      const navigate = () => {
        reset(`RELOAD${++reloads}`);
        loadFired = false;
        for (const peer of sessions.keys()) {
          for (const page of localPages(peer)) {
            beforeNavigateFrame(peer, page, event, sessions);
            event('Page.frameNavigated', { frame: frame(page) }, peer);
          }
          event('Runtime.executionContextsCleared', {}, peer);
          created(peer);
          for (const page of localPages(peer))
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
      };
      navigateLatest = navigate;
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
        const target = sessions.get(sessionId) || 'TAB';
        answer({ frameTree: target === 'TAB' ? tree() : { frame: frame(pages.get(target)) } });
        return true;
      }
      if (method === 'Target.setAutoAttach') {
        if (params.autoAttach && sessions.get(sessionId) === 'TAB') {
          autoParents.add(sessionId);
          if (!attachDuringRearm) attachOopifs(sessionId);
        }
        answer({});
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
        answer({
          root: {
            nodeId: 100,
            nodeType: 9,
            childNodeCount: children.length,
            children: children.map((child, index) => ({
              nodeId: index + 1,
              nodeType: 1,
              nodeName: 'IFRAME',
              frameId: child.id,
              childNodeCount: 0,
            })),
          },
        });
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
      if (method === 'Debugger.setInstrumentationBreakpoint') {
        scripts.set(sessionId, `instrument-${sessionId}`);
        answer({ breakpointId: `instrument-${sessionId}` });
        return true;
      }
      if (method === 'Debugger.removeBreakpoint' || method === 'Debugger.disable') {
        scripts.delete(sessionId);
        answer({});
        return true;
      }
      if (method === 'Page.addScriptToEvaluateOnNewDocument') {
        if (attachDuringRearm && !params.runImmediately && autoParents.has(sessionId))
          attachOopifs(sessionId);
        scripts.set(sessionId, params.source);
        if (
          installerWaitsForResume &&
          params.runImmediately &&
          pages.get(sessions.get(sessionId))?.oopif
        ) {
          pendingInstallers.set(sessionId, () => answer({ identifier: sessionId }));
          begin(sessionId);
        } else {
          answer({ identifier: sessionId });
          if (params.runImmediately) begin(sessionId);
        }
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
        if (installerWaitsForResume && [...pages.values()].some((page) => !page.loaded)) return;
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
        if (page?.document.modelContext && !page.loader.startsWith('L-') && !page.loaded) {
          page.loaded = true;
          for (const definition of page.definitions || [])
            page.document.modelContext?.registerTool?.(definition);
        }
        const installed = pendingInstallers.get(sessionId);
        pendingInstallers.delete(sessionId);
        installed?.();
        if (reentrantPauses && queues.get(sessionId)?.length) {
          pause(sessionId);
          answer({});
          return true;
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
      if (method === 'Runtime.runIfWaitingForDebugger') {
        const page = pages.get(sessions.get(sessionId));
        if (page?.blank && page.oopif && scripts.has(sessionId)) {
          page.blank = false;
          for (const peer of sessions.keys()) {
            if (sessions.get(peer) !== page.id) continue;
            event('Page.frameNavigated', { frame: frame(page) }, peer);
            event('Runtime.executionContextsCleared', {}, peer);
            created(peer);
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
          }
          begin(sessionId);
        }
        answer({});
        return true;
      }
      if (method === 'Page.reload') {
        answer({});
        navigate();
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
  for (const child of children.filter((child) => child.oopif))
    fake.targets.push({
      targetId: child.id,
      type: 'iframe',
      parentFrameId: 'TAB',
      url: `${child.origin}/`,
    });
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
    reload() {
      navigateLatest();
    },
    abandonLocalGate(frameId) {
      for (const [peer, frames] of queues)
        queues.set(
          peer,
          frames.filter((frame) => frame !== frameId)
        );
    },
    inject(message) {
      writers.get(message.sessionId)(message);
    },
  };
}
