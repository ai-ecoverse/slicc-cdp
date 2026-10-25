import assert from 'node:assert/strict';

export async function openStoragePage(raw, url) {
  const { targetId } = await raw.send('Target.createTarget', {
    url: 'about:blank',
    background: true,
  });
  const { sessionId } = await raw.send('Target.attachToTarget', { targetId, flatten: true });
  let committed;
  let loaded;
  try {
    await raw.send('Page.enable', {}, sessionId);
    await raw.send('Page.setLifecycleEventsEnabled', { enabled: true }, sessionId);
    let loader;
    const loads = new Set();
    committed = raw.event(
      'Page.frameNavigated',
      sessionId,
      ({ frame }) => !frame.parentId && frame.url === url
    );
    loaded = raw.event('Page.lifecycleEvent', sessionId, (event) => {
      if (event.name !== 'load') return false;
      loads.add(event.loaderId);
      return event.loaderId === loader;
    });
    let timer;
    try {
      await Promise.race([
        (async () => {
          const result = await raw.send('Page.navigate', { url }, sessionId);
          assert.equal(result.errorText, undefined);
          const { frame } = await committed.seen;
          assert.equal(frame.unreachableUrl, undefined);
          assert.ok(frame.loaderId);
          loader = frame.loaderId;
          if (!loads.has(loader)) await loaded.seen;
        })(),
        new Promise((_, reject) => {
          timer = setTimeout(
            () => reject(new Error('storage fixture document did not commit and load')),
            15000
          );
        }),
      ]);
    } finally {
      clearTimeout(timer);
    }
    return targetId;
  } finally {
    committed?.cancel();
    loaded?.cancel();
    await raw.send('Target.detachFromTarget', { sessionId });
  }
}
