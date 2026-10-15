import { rawSocket } from './browser.mjs';

export async function plantOrigins(wsUrl, origins, cookies = ['canary', 'del']) {
  const raw = await rawSocket(wsUrl);
  try {
    for (const origin of origins) await plantOne(raw, origin, cookies);
  } finally {
    raw.close();
  }
}

async function plantOne(raw, origin, cookies) {
  const url = `${origin}/plant`;
  const host = new URL(origin).hostname;
  const { targetId } = await raw.send('Target.createTarget', {
    url: 'about:blank',
    background: true,
  });
  try {
    const { sessionId } = await raw.send('Target.attachToTarget', { targetId, flatten: true });
    await raw.send('Network.enable', {}, sessionId);
    await raw.send('Page.enable', {}, sessionId);
    const response = raw.event(
      'Network.responseReceived',
      sessionId,
      (params) => params.type === 'Document' && params.response?.url === url
    );
    const loaded = raw.event('Page.loadEventFired', sessionId);
    const navigated = await raw.send('Page.navigate', { url }, sessionId);
    if (navigated.errorText) throw new Error(`${url} did not load: ${navigated.errorText}`);
    await response.seen;
    await loaded.seen;
    const { cookies: jar } = await raw.send('Storage.getCookies', {});
    const names = jar.filter((cookie) => cookie.domain === host).map((cookie) => cookie.name);
    const missing = cookies.filter((name) => !names.includes(name));
    if (missing.length) {
      throw new Error(`${url} responded but left no ${missing.join(', ')} cookie for ${host}`);
    }
  } finally {
    await raw.send('Target.closeTarget', { targetId });
  }
}
