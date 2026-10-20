import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';

const bridge = fileURLToPath(new URL('../../target/debug/examples/guard_bridge', import.meta.url));

function channel(write, onMessage) {
  const events = [];
  const pending = new Map();
  let id = 0;
  const receive = (message) => {
    onMessage?.(message);
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

async function wasiGuarded(ws, origin, { variant, detachDelay, onMessage }) {
  const { createNodeKernel } = await import('@ai-ecoverse/slicc-kernel/node');
  const { bridge: browserBridge } = await import('./browser.mjs');
  const kernel = await createNodeKernel({ cdp: browserBridge(ws) });
  let terminal;
  let client;
  let closing = false;
  try {
    await kernel.writeFile(
      '/node_modules/guard/package.json',
      JSON.stringify({
        name: 'guard',
        slicc: {
          commands: {
            'guard-bridge': { abi: 'wasi', wasm: 'bridge.wasm' },
            'guard-terminal': { abi: 'js', module: 'terminal.mjs' },
          },
        },
      })
    );
    await kernel.writeFile(
      '/node_modules/guard/bridge.wasm',
      await readFile(
        new URL('../../target/wasm32-wasip1/debug/examples/guard_bridge.wasm', import.meta.url)
      )
    );
    await kernel.writeFile(
      '/node_modules/guard/terminal.mjs',
      `export default async function(ctx) {
      await ctx.tty.setRaw(0, true);
      const child = await ctx.spawn({ argv: ['guard-bridge', ...ctx.argv.slice(1)] });
      await ctx.write(1, '{"ready":true}\\n');
      return (await child.wait()).status;
    }`
    );
    client = channel((message) => terminal.write(`${message}\n`), onMessage);
    let buffered = '';
    let ready;
    const started = new Promise((resolve) => {
      ready = resolve;
    });
    terminal = await kernel.openTerminal(['guard-terminal', ws, origin], {
      onData: (bytes) => {
        if (closing) return;
        buffered += new TextDecoder().decode(bytes);
        for (;;) {
          const end = buffered.indexOf('\n');
          if (end < 0) break;
          const line = buffered.slice(0, end).replaceAll('\r', '');
          buffered = buffered.slice(end + 1);
          let message;
          try {
            message = JSON.parse(line);
          } catch {
            throw new Error(`invalid WASI guard bridge output: ${line}`);
          }
          if (message.ready) ready();
          else client.receive(message);
        }
      },
    });
    terminal.ended.catch(client.stop);
    terminal.exited.then(client.stop, client.stop);
    let timer;
    try {
      await Promise.race([
        started,
        new Promise((_, reject) => {
          timer = setTimeout(() => reject(new Error('WASI guard bridge startup timed out')), 10000);
        }),
      ]);
    } finally {
      clearTimeout(timer);
    }
    return {
      events: client.events,
      send: client.send,
      waitEvent: client.waitEvent,
      variant,
      detachDelay,
      close: async () => {
        closing = true;
        terminal.signal('SIGKILL');
        await terminal.exited.catch(() => {});
        client.stop();
        kernel.terminate();
      },
    };
  } catch (error) {
    closing = true;
    terminal?.signal('SIGKILL');
    client?.stop();
    kernel.terminate();
    throw error;
  }
}

export async function guarded(ws, origin, { variant, detachDelay, onMessage }) {
  if (!['prompt', 'lagging'].includes(variant) || !Number.isInteger(detachDelay))
    throw new Error('invalid guard variant');
  if (process.env.SLICC_GUARD_TRANSPORT === 'wasi')
    return wasiGuarded(ws, origin, { variant, detachDelay, onMessage });
  const child = spawn(bridge, [ws, origin], { stdio: ['pipe', 'pipe', 'inherit'] });
  const client = channel((message) => child.stdin.write(`${message}\n`), onMessage);
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

export async function observer(wsUrl, onMessage) {
  const ws = new WebSocket(wsUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener('open', resolve, { once: true });
    ws.addEventListener('error', reject, { once: true });
  });
  const client = channel((message) => ws.send(message), onMessage);
  ws.addEventListener('message', (event) => client.receive(JSON.parse(event.data)));
  ws.addEventListener('close', client.stop);
  return {
    events: client.events,
    send: client.send,
    waitEvent: client.waitEvent,
    close: async () => {
      if (ws.readyState === WebSocket.CLOSED) return;
      const closed = new Promise((resolve) =>
        ws.addEventListener('close', resolve, { once: true })
      );
      ws.close();
      await closed;
    },
  };
}
