import { protectedOrigins } from './protected.js';
import { PROTECTED_ORIGINS } from './protected-origins.js';

export const REFUSAL = PROTECTED_ORIGINS.refusal;

const guard = protectedOrigins(PROTECTED_ORIGINS);

function under(host, domain) {
  return host === domain || host.endsWith(`.${domain}`);
}

function inheritingAbout(text) {
  if (typeof text !== 'string') return false;
  try {
    const url = new URL(text);
    if (url.protocol !== 'about:') return false;
    return PROTECTED_ORIGINS.inheriting.includes(`about:${url.pathname.toLowerCase()}`);
  } catch {
    return false;
  }
}

export function isProtectedHost(host) {
  if (typeof host !== 'string' || host.length === 0) return true;
  const name = host.toLowerCase().replace(/\.$/u, '');
  if (PROTECTED_ORIGINS.hosts.includes(name)) return true;
  return PROTECTED_ORIGINS.domains.some((domain) => under(name, domain));
}

export function isProtectedUrl(text) {
  if (typeof text !== 'string' || text.length === 0) return true;
  if (inheritingAbout(text)) return true;
  if (
    typeof PROTECTED_ORIGINS.errorPage?.url === 'string' &&
    text === PROTECTED_ORIGINS.errorPage.url
  ) {
    return true;
  }
  if (text.startsWith('chrome-error:')) return true;
  return guard.url(text);
}

export function refuseProtected(text) {
  if (isProtectedUrl(text)) throw new Error(REFUSAL);
}
