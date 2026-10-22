import { cdpProxy } from './cdp-proxy.mjs';
import { recordedClient } from './guard-c2-support.mjs';
import { guarded, observer } from './guard-client.mjs';
import { InboundGate } from './inbound-gate.mjs';

export async function inboundClients(
  wsUrl,
  origin,
  { variant, detachDelay, protectedOrigin, ordinaryOrigin }
) {
  const controlGate = new InboundGate({ protectedOrigin, ordinaryOrigin });
  const guardedGate = new InboundGate({
    protectedOrigin,
    ordinaryOrigin,
    seedSource: (targetId) => controlGate.observedSeed(targetId),
  });
  const guardedProxy = await cdpProxy(wsUrl, { inbound: guardedGate });
  let controlProxy;
  let client;
  let unguarded;
  let closed;
  const close = () =>
    (closed ??= (async () => {
      await Promise.all([guardedProxy.close(), controlProxy?.close()]);
      await Promise.all([client?.close(), unguarded?.close()]);
    })());
  try {
    controlProxy = await cdpProxy(wsUrl, { inbound: controlGate });
    client = await recordedClient((onMessage) =>
      guarded(guardedProxy.url, origin, { variant, detachDelay, onMessage })
    );
    unguarded = await recordedClient((onMessage) => observer(controlProxy.url, onMessage));
    client.inboundDelay = (args) => {
      const expected = args.side === 'control' ? unguarded : client;
      if (args.client?.events !== expected.events) throw new Error('inbound gate client mismatch');
      return (args.side === 'control' ? controlGate : guardedGate).arm(args);
    };
    return { client, unguarded, close };
  } catch (error) {
    await close();
    throw error;
  }
}
