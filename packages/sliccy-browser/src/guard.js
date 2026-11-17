import { protectedOrigins } from './protected.js';
import { PROTECTED_ORIGINS } from './protected-origins.js';

export const REFUSAL = PROTECTED_ORIGINS.refusal;

const guard = protectedOrigins(PROTECTED_ORIGINS);

export const CREDENTIAL_ORIGINS = [
  'https://ims-na1.adobelogin.com',
  'https://ims-na1-stg1.adobelogin.com',
];

const credentialHosts = CREDENTIAL_ORIGINS.map((origin) => new URL(origin).hostname);

function coversCredential(domain) {
  return domain.length > 0 && credentialHosts.some((host) => under(host, domain));
}

function credentialDomain(text) {
  const domain = text.toLowerCase().replace(/^\./u, '').replace(/\.$/u, '');
  return coversCredential(domain);
}

function credentialSite(text) {
  try {
    return coversCredential(hostOf(text));
  } catch {
    return false;
  }
}

function credentialUrl(text) {
  let host;
  try {
    host = hostOf(asHttp(text));
  } catch {
    return false;
  }
  if (host.startsWith('[') || /^[0-9.]+$/u.test(host)) return false;
  const labels = host.split('.');
  for (let at = 0; at < labels.length - 1; at += 1) {
    if (coversCredential(labels.slice(at).join('.'))) return true;
  }
  return false;
}

export function credentialCookie(cookie) {
  if (!cookie || typeof cookie !== 'object') return true;
  if (typeof cookie.domain === 'string' && credentialDomain(cookie.domain)) return true;
  if (typeof cookie.url === 'string' && credentialUrl(cookie.url)) return true;
  const key = cookie.partitionKey;
  if (typeof key === 'string') return credentialSite(key);
  if (key && typeof key === 'object' && typeof key.topLevelSite === 'string') {
    return credentialSite(key.topLevelSite);
  }
  return false;
}

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

function hostOf(text) {
  const host = new URL(text).hostname.toLowerCase();
  return host.endsWith('.') ? host.slice(0, -1) : host;
}

function asHttp(text) {
  try {
    const url = new URL(text);
    if (url.protocol === 'ws:') url.protocol = 'http:';
    else if (url.protocol === 'wss:') url.protocol = 'https:';
    return url.href;
  } catch {
    return text;
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
  const mapped = asHttp(text);
  return guard.url(text) || (mapped !== text && guard.url(mapped));
}

function siteProtected(site) {
  if (typeof site !== 'string' || site.length === 0) return true;
  if (isProtectedUrl(site)) return true;
  const host = hostOf(site);
  return host.length > 0 && guard.cookieDomain(host);
}

export function cookieProtected(cookie) {
  if (!cookie || typeof cookie !== 'object') return true;
  if (credentialCookie(cookie)) return true;
  if (typeof cookie.domain === 'string' && guard.cookieDomain(cookie.domain)) return true;
  if (typeof cookie.url === 'string') {
    if (isProtectedUrl(cookie.url)) return true;
    const host = hostOf(cookie.url);
    if (host.length > 0 && guard.cookieDomain(host)) return true;
  }
  const key = cookie.partitionKey;
  if (typeof key === 'string') return siteProtected(key);
  if (key && typeof key === 'object' && typeof key.topLevelSite === 'string') {
    return siteProtected(key.topLevelSite);
  }
  return false;
}

export function dropProtectedCookies(cookies) {
  if (!Array.isArray(cookies)) return [];
  return cookies.filter((cookie) => !cookieProtected(cookie));
}

export function refuseProtected(text) {
  if (isProtectedUrl(text)) throw new Error(REFUSAL);
}
