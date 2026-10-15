import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';

const bridge = fileURLToPath(new URL('../../target/debug/examples/guard_bridge', import.meta.url));

function channel(write) {
  const events = [];
  const pending = new Map();
  let id = 0;
  const receive = (message) => {
    if (message.event) events.push(message.event);
    else if (message.method) events.push(message);
    const waiter = pending.get(message.id);
    if (!waiter) return;
    clearTimeout(waiter.timer);
    pending.delete(message.id);
    if (message.error) {
      const error = new Error(message.error.message);
      error.cdp = message.error;
      waiter.reject(error);
    } else waiter.resolve(message.result ?? {});
  };
  const stop = () => {
    for (const waiter of pending.values()) {
      clearTimeout(waiter.timer);
      waiter.reject(new Error('CDP connection closed'));
    }
    pending.clear();
  };
  return {
    receive,
    stop,
    events,
    send: (method, params = {}, sessionId) =>
      new Promise((resolve, reject) => {
        const seq = ++id;
        const timer = setTimeout(() => {
          pending.delete(seq);
          reject(new Error(`CDP timeout ${method}`));
        }, 35000);
        pending.set(seq, { resolve, reject, timer });
        write(JSON.stringify({ id: seq, method, params, sessionId }));
      }),
    waitEvent: (pred, ms) =>
      new Promise((resolve, reject) => {
        const from = events.length;
        const deadline = Date.now() + ms;
        const poll = () => {
          const found = events.slice(from).find(pred);
          if (found) return resolve(found);
          if (Date.now() >= deadline) return reject(new Error('CDP event timeout'));
          setTimeout(poll, 10);
        };
        poll();
      }),
  };
}

export async function guarded(ws, origin, { variant, detachDelay }) {
  if (!['prompt', 'lagging'].includes(variant) || !Number.isInteger(detachDelay))
    throw new Error('invalid guard variant');
  const child = spawn(bridge, [ws, origin], { stdio: ['pipe', 'pipe', 'inherit'] });
  const client = channel((message) => child.stdin.write(`${message}\n`));
  createInterface({ input: child.stdout }).on('line', (line) => client.receive(JSON.parse(line)));
  child.on('error', client.stop);
  child.stdin.on('error', client.stop);
  const closed = new Promise((resolve) => child.once('close', resolve));
  child.once('close', client.stop);
  return {
    events: client.events,
    send: client.send,
    waitEvent: client.waitEvent,
    variant,
    detachDelay,
    close: async () => {
      child.stdin.end();
      await closed;
    },
  };
}

export async function observer(wsUrl) {
  const ws = new WebSocket(wsUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener('open', resolve, { once: true });
    ws.addEventListener('error', reject, { once: true });
  });
  const client = channel((message) => ws.send(message));
  ws.addEventListener('message', (event) => client.receive(JSON.parse(event.data)));
  ws.addEventListener('close', client.stop);
  return {
    events: client.events,
    send: client.send,
    waitEvent: client.waitEvent,
    close: async () => {
      const closed = new Promise((resolve) =>
        ws.addEventListener('close', resolve, { once: true })
      );
      ws.close();
      await closed;
    },
  };
}
