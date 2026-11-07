const parse = (text) => {
  if (typeof text !== 'string') return null;
  try {
    return new URL(text);
  } catch {
    return null;
  }
};

const scheme = (u) => u.protocol.slice(0, -1);

const hostOf = (u) => {
  const h = u.hostname.toLowerCase();
  return h.endsWith('.') ? h.slice(0, -1) : h;
};

const originOf = (u) => {
  if (!u?.host) return null;
  return `${scheme(u)}://${hostOf(u)}${u.port ? `:${u.port}` : ''}`;
};

const under = (host, domain) => host === domain || host.endsWith(`.${domain}`);

export function protectedOrigins(rules, extraOrigins = []) {
  const extra = extraOrigins.map((o) => originOf(parse(o))).filter(Boolean);
  const extraHosts = extraOrigins
    .map((o) => parse(o))
    .filter((u) => u?.host)
    .map(hostOf);

  const unwrap = (u) => {
    for (let i = 0; u && rules.innerSchemes.includes(scheme(u)) && i < 8; i += 1) {
      u = parse(u.href.slice(u.protocol.length));
    }
    return u;
  };

  const alias = (u) => {
    if (!u || scheme(u) !== 'about' || !rules.aboutScheme) return u;
    if (rules.inheriting.includes(`about:${u.pathname.toLowerCase()}`)) return u;
    return parse(`${rules.aboutScheme}://${u.pathname}${u.search}${u.hash}`) ?? u;
  };

  const exempt = (text) => text === '' || text === 'null' || text === '://';
  const failed = (u) => !u || rules.innerSchemes.includes(scheme(u));
  const unparseable = (text) => !exempt(text) && failed(unwrap(parse(text)));

  const url = (text, { readOnly = false } = {}) => {
    const u = alias(unwrap(parse(text)));
    if (failed(u)) return rules.unparseable === 'protected' && !readOnly && !exempt(text);
    const s = scheme(u);
    if (rules.anySchemes.includes(s)) return true;
    if ((rules.protectedSchemes ?? []).includes(s))
      return !(rules.schemeExceptions ?? []).includes(`${s}://${hostOf(u)}`);
    if (rules.schemes.includes(s) && rules.hosts.includes(hostOf(u))) return true;
    if (rules.schemes.includes(s) && rules.domains.some((d) => under(hostOf(u), d))) return true;
    const o = originOf(u);
    return o !== null && extra.includes(o);
  };

  const inheriting = (text) => {
    if (text === '') return true;
    const u = parse(text);
    return (
      u !== null &&
      scheme(u) === 'about' &&
      rules.inheriting.includes(`about:${u.pathname.toLowerCase()}`)
    );
  };

  const target = ({
    url: text,
    previous = false,
    opener,
    type,
    documentURL,
    unreachableURL,
    navigating = false,
  }) => {
    if (type && (rules.targetTypes ?? []).includes(type)) return true;
    if (
      rules.errorPage &&
      documentURL === rules.errorPage.url &&
      unreachableURL === text &&
      !navigating
    )
      return previous;
    if (inheriting(text)) return previous || opener === 'protected' || opener === 'unknown';
    return url(text);
  };

  const cookieDomain = (domain) => {
    let d = domain.toLowerCase();
    if (d.startsWith('.')) d = d.slice(1);
    if (d.endsWith('.')) d = d.slice(0, -1);
    if (d === '') return false;
    if (rules.domains.some((x) => under(d, x))) return true;
    return [...rules.domains, ...rules.hosts, ...extraHosts].some((h) => under(h, d));
  };

  const storageKey = (key) => {
    const [first, ...rest] = key.split('^');
    return [first, ...rest.filter((p) => p.startsWith('0')).map((p) => p.slice(1))].some((text) =>
      url(text, { readOnly: true })
    );
  };

  const extensionFrame = (text) => {
    const u = parse(text);
    if (!u || !(rules.extensionFrames?.schemes ?? []).includes(scheme(u))) return false;
    return !extra.includes(originOf(u));
  };

  return { url, target, cookieDomain, storageKey, unparseable, extensionFrame };
}
