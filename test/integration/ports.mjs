import net from 'node:net';

export const HARNESS_PORTS = '9300-9309';

if (!process.env.SLICC_CDP_PORTS) process.env.SLICC_CDP_PORTS = HARNESS_PORTS;

export function realDebugPort(host, port) {
  const number = Number(port);
  if (!Number.isInteger(number) || number < 9222 || number > 9229) return false;
  const name = String(host ?? 'localhost')
    .toLowerCase()
    .replace(/^\[|\]$/g, '')
    .replace(/\.$/, '');
  if (name === '' || name === 'localhost' || name.endsWith('.localhost')) return true;
  if (name === '::' || name === '::1' || name === '0.0.0.0') return true;
  const v4 = name.replace(/^::ffff:/, '');
  return net.isIPv4(v4) && (v4.startsWith('127.') || v4 === '0.0.0.0');
}

function target(args) {
  const [first, second] = args;
  if (Array.isArray(first)) return target(first);
  if (first && typeof first === 'object') {
    if (first.path) return null;
    return { host: first.host, port: first.port };
  }
  if (typeof first === 'string' && !/^\d+$/.test(first)) return null;
  return { host: typeof second === 'string' ? second : undefined, port: first };
}

const connect = net.Socket.prototype.connect;
if (!connect.sliccGuarded) {
  const guarded = function (...args) {
    const to = target(args);
    if (to && realDebugPort(to.host, to.port)) {
      throw new Error(`test connected to ${to.host ?? 'localhost'}:${to.port}`);
    }
    return connect.apply(this, args);
  };
  guarded.sliccGuarded = true;
  net.Socket.prototype.connect = guarded;
}
