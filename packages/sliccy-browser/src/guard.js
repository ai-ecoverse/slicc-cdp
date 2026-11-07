export const REFUSAL = "slicc: refused: target is on seven's origin";

function parse(text) {
  if (typeof text !== 'string') return null;
  try {
    return new URL(text);
  } catch {
    return null;
  }
}

function hostOf(url) {
  const host = url.hostname.toLowerCase();
  return host.endsWith('.') ? host.slice(0, -1) : host;
}

function under(host, domain) {
  return host === domain || host.endsWith(`.${domain}`);
}

export function isProtectedHost(host) {
  if (typeof host !== 'string' || host.length === 0) return true;
  const name = host.toLowerCase().replace(/\.$/u, '');
  return under(name, 'sliccy.ai');
}

export function isProtectedUrl(text) {
  if (typeof text !== 'string' || text.length === 0) return true;
  const url = parse(text);
  if (!url) return true;
  const scheme = url.protocol.slice(0, -1);
  if (scheme === 'chrome-extension' || scheme === 'chrome-error') return true;
  if (scheme === 'https' || scheme === 'http') return isProtectedHost(hostOf(url));
  return false;
}

export function refuseProtected(text) {
  if (isProtectedUrl(text)) throw new Error(REFUSAL);
}
