(() => {
  if (
    !isSecureContext ||
    'modelContext' in Document.prototype ||
    'modelContext' in document ||
    'modelContext' in navigator
  )
    return;
  const apply = Reflect.apply;
  const define = Object.defineProperty;
  const descriptor = Object.getOwnPropertyDescriptor;
  const parse = JSON.parse;
  const stringify = JSON.stringify;
  const isArray = Array.isArray;
  const then = Promise.prototype.then;
  const StringValue = String;
  const ErrorValue = DOMException;
  const Controller = AbortController;
  const PromiseValue = Promise;
  const resolve = Promise.resolve.bind(Promise);
  const mapGet = Map.prototype.get;
  const mapSet = Map.prototype.set;
  const mapDelete = Map.prototype.delete;
  const mapValues = Map.prototype.values;
  const iteratorNext = Object.getPrototypeOf(new Map().values()).next;
  const push = Array.prototype.push;
  const sort = Array.prototype.sort;
  const slice = String.prototype.slice;
  const add = EventTarget.prototype.addEventListener;
  const remove = EventTarget.prototype.removeEventListener;
  const dispatch = EventTarget.prototype.dispatchEvent;
  const EventValue = Event;
  const abort = AbortController.prototype.abort;
  const ownOrigin = location.origin;
  const registry = new Map();
  const context = new EventTarget();
  const cap = 65536;
  let changes = 0;
  const bounded = (value) => apply(slice, StringValue(value), [0, cap]);
  const copy = (value) => parse(stringify(value));
  const entries = () => {
    const result = [];
    const iterator = apply(mapValues, registry, []);
    for (;;) {
      const entry = apply(iteratorNext, iterator, []);
      if (entry.done) break;
      apply(push, result, [entry.value]);
    }
    return result;
  };
  const changed = () => {
    changes += 1;
    const event = new EventValue('toolchange');
    apply(dispatch, context, [event]);
    if (typeof context.ontoolchange === 'function') context.ontoolchange(event);
  };
  const metadata = (entry, bridge) => {
    const tool = copy(entry.metadata);
    if (bridge) tool.exposedTo = copy(entry.exposedTo);
    else tool.window = window;
    return tool;
  };
  const localTools = (bridge) => {
    const tools = [];
    const records = entries();
    for (let index = 0; index < records.length; index += 1)
      apply(push, tools, [metadata(records[index], bridge)]);
    apply(sort, tools, [(a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0)]);
    return tools;
  };
  const unregister = (name, entry) => {
    if (apply(mapGet, registry, [name]) !== entry) return;
    apply(mapDelete, registry, [name]);
    if (entry.signal) apply(remove, entry.signal, ['abort', entry.onabort]);
    changed();
  };
  const registerTool = (tool, options = {}) => {
    if (!tool || typeof tool.name !== 'string' || !tool.name || typeof tool.execute !== 'function')
      throw new TypeError('A tool needs a name and execute function');
    if (apply(mapGet, registry, [tool.name]))
      throw new ErrorValue('Duplicate tool name', 'InvalidStateError');
    if (options.signal?.aborted) return;
    const schema = tool.inputSchema ?? { type: 'object' };
    if (typeof schema !== 'object' || schema === null || isArray(schema))
      throw new TypeError('inputSchema must be an object');
    const annotations = copy(tool.annotations ?? {});
    if (annotations.readOnly !== undefined) annotations.readOnlyHint = annotations.readOnly;
    if (annotations.consequential !== undefined)
      annotations.consequentialHint = annotations.consequential;
    if (annotations.untrustedContent !== undefined)
      annotations.untrustedContentHint = annotations.untrustedContent;
    const entry = {
      execute: tool.execute,
      metadata: {
        name: tool.name,
        title: tool.title ?? null,
        description: tool.description ?? '',
        inputSchema: copy(schema),
        annotations,
        origin: ownOrigin,
      },
      exposedTo: copy(tool.exposedTo ?? []),
      signal: options.signal,
    };
    entry.onabort = () => unregister(entry.metadata.name, entry);
    if (entry.signal) apply(add, entry.signal, ['abort', entry.onabort, { once: true }]);
    apply(mapSet, registry, [entry.metadata.name, entry]);
    changed();
  };
  const getTools = async () => {
    const tools = localTools(false);
    for (let index = 0; index < window.frames.length; index += 1) {
      try {
        const child = window.frames[index];
        if (child.location.origin !== ownOrigin || !child.document.modelContext) continue;
        const nested = await child.document.modelContext.getTools();
        for (let at = 0; at < nested.length; at += 1) apply(push, tools, [nested[at]]);
      } catch {}
    }
    apply(sort, tools, [(a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0)]);
    return tools;
  };
  const executeTool = async (tool, input, options = {}) => {
    if (tool?.window && tool.window !== window) {
      if (tool.window.location.origin !== ownOrigin)
        throw new ErrorValue('Cross-origin tool access is not allowed', 'SecurityError');
      return tool.window.document.modelContext.executeTool(tool, input, options);
    }
    const name = typeof tool === 'string' ? tool : tool?.name;
    const entry = apply(mapGet, registry, [name]);
    if (!entry) throw new ErrorValue('Unknown tool', 'NotFoundError');
    const signal = options.signal;
    if (signal?.aborted) throw new ErrorValue('Tool invocation aborted', 'AbortError');
    const controller = new Controller();
    let onabort;
    try {
      return await new PromiseValue((fulfill, reject) => {
        onabort = () => {
          apply(abort, controller, []);
          reject(new ErrorValue('Tool invocation aborted', 'AbortError'));
        };
        if (signal) apply(add, signal, ['abort', onabort, { once: true }]);
        const parsed = typeof input === 'string' ? parse(input) : input;
        if (!parsed || typeof parsed !== 'object' || isArray(parsed))
          throw new TypeError('Tool input must be an object');
        apply(then, resolve(entry.execute(parsed, { signal: controller.signal })), [
          (result) => fulfill(typeof result === 'string' ? result : stringify(result ?? null)),
          (error) => reject(new ErrorValue(bounded(error), 'UnknownError')),
        ]);
      });
    } catch (error) {
      if (error?.name === 'AbortError' || error?.name === 'UnknownError') throw error;
      throw new ErrorValue(bounded(error), 'UnknownError');
    } finally {
      if (signal && onabort) apply(remove, signal, ['abort', onabort]);
    }
  };
  define(context, '__slicc_webmcp_shim', { value: true });
  define(context, Symbol.toStringTag, { value: 'ModelContext' });
  define(context, 'registerTool', { value: registerTool });
  define(context, 'getTools', { value: getTools });
  define(context, 'executeTool', { value: executeTool });
  context.ontoolchange = null;
  const alias = (owner, prototype) => {
    define(owner, 'modelContext', {
      configurable: true,
      get() {
        const native = descriptor(prototype, 'modelContext');
        return native?.get ? apply(native.get, this, []) : context;
      },
      set(value) {
        define(owner, 'modelContext', { configurable: true, writable: true, value });
      },
    });
  };
  alias(document, Document.prototype);
  alias(navigator, Navigator.prototype);
  return {
    list() {
      const tools = stringify(localTools(true));
      if (tools.length > cap) return { error: 'limit' };
      return { toolsJson: tools, changes, replaced: document.modelContext !== context };
    },
    async call(name, input, allowConsequential) {
      const entry = apply(mapGet, registry, [name]);
      if (!entry) return { __webmcp_error: 'unknown' };
      if (!allowConsequential && entry.metadata.annotations.consequentialHint)
        return {
          __webmcp_error: 'consequential',
          description: bounded(entry.metadata.description),
        };
      try {
        return { __webmcp_result: bounded(await executeTool(name, input)) };
      } catch (error) {
        return { __webmcp_error: 'threw', message: bounded(error) };
      }
    },
  };
})();
