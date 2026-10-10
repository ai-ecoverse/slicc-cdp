import { CDPClient } from './cdp-client.js';

const LOAD_TIMEOUT_MS = 15000;
const PROBE_TIMEOUT_MS = 1000;

export class PageSession {
  constructor(client) {
    this.client = client ?? new CDPClient();
    this.sessionId = null;
  }

  async listTargets() {
    const result = await this.client.send('Target.getTargets', {});
    const infos = Array.isArray(result?.targetInfos) ? result.targetInfos : [];
    const pages = [];
    for (const info of infos) {
      if (info?.type !== 'page') continue;
      const targetId = typeof info.targetId === 'string' ? info.targetId : '';
      const url = typeof info.url === 'string' ? info.url : '';
      const title = typeof info.title === 'string' ? info.title : '';
      if (targetId === '' || isInternalTarget(url, title)) continue;
      pages.push({ targetId, url, title });
    }
    return pages;
  }

  async attach(targetId) {
    const result = await this.client.send('Target.attachToTarget', {
      targetId,
      flatten: true,
    });
    const sessionId = result?.sessionId;
    if (typeof sessionId !== 'string' || sessionId === '') {
      throw new Error('Target.attachToTarget returned no sessionId');
    }
    this.sessionId = sessionId;
    await enablePage(this.client, sessionId);
    return sessionId;
  }

  async navigate(url) {
    const sessionId = this.requireSession();
    await enablePage(this.client, sessionId);
    const result = await this.client.send('Page.navigate', { url }, sessionId);
    if (typeof result?.errorText === 'string' && result.errorText !== '') {
      throw new Error(`navigate: ${result.errorText}`);
    }
    if (await pageIsSettled(this.client, sessionId)) return;
    try {
      await this.client.once('Page.loadEventFired', LOAD_TIMEOUT_MS);
    } catch (err) {
      if (!isLoadTimeout(err)) throw err;
    }
  }

  async evaluate(expression, options = {}) {
    const sessionId = this.requireSession();
    const result = await this.client.send(
      'Runtime.evaluate',
      {
        expression,
        returnByValue: options.returnByValue ?? true,
        awaitPromise: options.awaitPromise ?? true,
      },
      sessionId
    );
    if (result?.exceptionDetails) {
      throw new Error(exceptionMessage(result.exceptionDetails));
    }
    return result?.result?.value;
  }

  async screenshot(options = {}) {
    const sessionId = this.requireSession();
    const params = { format: 'png' };
    if (options.fullPage) params.captureBeyondViewport = true;
    if (options.clip) params.clip = options.clip;
    const result = await this.client.send('Page.captureScreenshot', params, sessionId);
    if (typeof result?.data !== 'string') {
      throw new Error('screenshot returned no data');
    }
    return result.data;
  }

  async accessibilitySnapshot(frameId) {
    const sessionId = this.requireSession();
    try {
      await this.client.send('Accessibility.enable', {}, sessionId);
    } catch {}
    const params = frameId ? { frameId } : {};
    const result = await this.client.send('Accessibility.getFullAXTree', params, sessionId);
    const nodes = Array.isArray(result?.nodes) ? result.nodes : [];
    return axFromCdp(nodes);
  }

  requireSession() {
    if (!this.sessionId) {
      throw new Error('Not attached. Call attach(targetId) first.');
    }
    return this.sessionId;
  }
}

function isInternalTarget(url, title) {
  const trimmedUrl = url.trim();
  const trimmedTitle = title.trim();
  if (trimmedTitle === 'Omnibox Popup') return true;
  if (trimmedUrl.startsWith('chrome://')) return true;
  if (trimmedUrl.startsWith('chrome-search://')) return true;
  if (trimmedUrl.startsWith('chrome-untrusted://')) return true;
  if (trimmedUrl.startsWith('devtools://')) return true;
  return trimmedUrl === '' && trimmedTitle.toLowerCase().endsWith('popup');
}

function enablePage(client, sessionId) {
  return client.send('Page.enable', {}, sessionId).catch((err) => {
    if (isCdpError(err)) return undefined;
    throw err;
  });
}

function isCdpError(err) {
  const message = err instanceof Error ? err.message : String(err);
  return message.startsWith('CDP error:');
}

function pageIsSettled(client, sessionId) {
  return client
    .send(
      'Runtime.evaluate',
      { expression: 'document.readyState', returnByValue: true },
      sessionId,
      PROBE_TIMEOUT_MS
    )
    .then((result) => {
      const state = result?.result?.value;
      return state !== 'loading' && state !== 'interactive';
    })
    .catch((err) => {
      const message = err instanceof Error ? err.message : String(err);
      if (
        message.startsWith('Runtime.evaluate timed out') ||
        message.startsWith('Runtime.evaluate:')
      ) {
        return true;
      }
      throw err;
    });
}

function isLoadTimeout(err) {
  const message = err instanceof Error ? err.message : String(err);
  return message.startsWith('Timed out waiting for event: Page.loadEventFired');
}

function exceptionMessage(details) {
  const description = details.exception?.description;
  if (typeof description === 'string') return description;
  if (typeof details.text === 'string') return details.text;
  return 'evaluation failed';
}

function axFromCdp(nodes) {
  const byId = new Map();
  for (const node of nodes) {
    if (node && typeof node.nodeId === 'string') byId.set(node.nodeId, node);
  }
  const roots = [];
  for (const node of nodes) {
    if (!node || typeof node.nodeId !== 'string') continue;
    if (node.parentId != null) continue;
    roots.push(...buildAxNode(node.nodeId, byId, new Set()));
  }
  return roots;
}

function buildAxNode(id, byId, stack) {
  if (stack.has(id)) return [];
  const node = byId.get(id);
  if (!node) return [];
  stack.add(id);
  const children = [];
  for (const childId of childIds(node)) {
    children.push(...buildAxNode(childId, byId, stack));
  }
  stack.delete(id);
  if (node.ignored === true) return children;
  const built = {
    role: textOf(fieldValue(node.role), 'unknown'),
    name: textOf(fieldValue(node.name), ''),
    value: textOf(fieldValue(node.value), ''),
    children,
  };
  if (typeof node.backendDOMNodeId === 'number') built.backendNodeId = node.backendDOMNodeId;
  return [built];
}

function fieldValue(field) {
  if (field == null) return undefined;
  if (typeof field === 'object' && 'value' in field) return field.value;
  return field;
}

function childIds(node) {
  if (!Array.isArray(node.childIds)) return [];
  return node.childIds.filter((id) => typeof id === 'string');
}

function textOf(value, fallback) {
  if (value == null) return fallback;
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean' || typeof value === 'bigint') {
    return String(value);
  }
  try {
    const json = JSON.stringify(value);
    return json ?? fallback;
  } catch {
    return fallback;
  }
}
