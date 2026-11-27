import assert from 'node:assert/strict';
import { createHash, generateKeyPairSync } from 'node:crypto';

export const origin = 'https://billing.example';
export const receipt = 'https://receipts.example/r/1042';
export const reason = "Open receipt; $(printf NOT_SHELL) '";
export const request = ['lease', 'request', '--allow', origin, '--for', '1h', '--reason', reason];
export const expansion = ['lease', 'request', '--from', 'L1', '--add', receipt, '--reason', reason];

export function fixture() {
  const { publicKey } = generateKeyPairSync('ed25519');
  const { crv, kty, x } = publicKey.export({ format: 'jwk' });
  const jkt = createHash('sha256').update(JSON.stringify({ crv, kty, x })).digest('base64url');
  const calls = [];
  const adapter = {
    async request(params) {
      calls.push(['request', params]);
      const n = params.lease ? 2 : 1;
      return { request: `R${n}`, lease: `L${n}`, jkt };
    },
    async action(params) {
      calls.push(['action', params]);
      if (params.lease === 'expired' || params.args.url === receipt) {
        throw Object.assign(new Error('secret redirect'), {
          reason: params.lease === 'expired' ? 'E_EXPIRED' : 'E_SCOPE',
        });
      }
      return { result: params.op === 'snapshot' ? { text: 'Invoice 1042\nbutton [ref=e12]' } : {} };
    },
  };
  return { adapter, jkt, calls };
}

export function refused(result, why, lease) {
  assert.equal(result.status, 1, `${result.stdout}\n${result.stderr}`);
  assert.equal(result.stdout, '');
  const err = JSON.parse(result.stderr);
  assert.equal(err.reason, why, result.stderr);
  if (lease) assert.equal(err.lease, lease);
  assert.ok(err.message);
  assert.doesNotMatch(result.stderr, /secret redirect|NOT_SHELL|PRIVATE/);
}

export const inputs = [
  ['click', 'e12'],
  ['fill', 'e13', '--', '--literal'],
];

export async function exercise(run, { jkt, calls }, { leaseKernel = false } = {}) {
  const pending = await run(request);
  assert.equal(pending.status, 0, pending.stderr);
  assert.equal(pending.stderr, '');
  assert.match(pending.stdout, /^Request: R1\nLease: L1\nPending owner review\./);
  assert.equal(
    pending.stdout.trimEnd().split('\n').at(-1),
    `slicc lease review --to jkt:${jkt} --allow ${origin} --for 1h`
  );
  assert.doesNotMatch(pending.stdout, /NOT_SHELL|PRIVATE/);
  for (const [op, ...args] of [['goto', origin], ...inputs]) {
    const r = await run(['--lease', 'L1', op, ...args]);
    if (leaseKernel && op !== 'goto') refused(r, 'E_UNSUPPORTED', 'L1');
    else assert.deepEqual([r.status, r.stdout, r.stderr], [0, 'OK\n', '']);
  }
  const snap = await run(['--lease=L1', 'snapshot']);
  assert.deepEqual(
    [snap.status, snap.stdout, snap.stderr],
    [0, 'Invoice 1042\nbutton [ref=e12]\n', '']
  );
  const blocked = await run(['--lease', 'L1', 'goto', receipt]);
  refused(blocked, 'E_SCOPE', 'L1');
  assert.equal(JSON.parse(blocked.stderr).outcome, 'unknown');
  const added = await run(expansion);
  assert.equal(added.status, 0, added.stderr);
  assert.equal(added.stdout, 'Request: R2\nLease: L2\nPending owner review.\n');
  refused(await run(['--lease', 'expired', 'snapshot']), 'E_EXPIRED', 'expired');
  for (const args of [
    ['--lease', 'L1', 'eval', '1'],
    ['--lease', 'L1', 'snapshot', '--tab', 'T'],
    ['--lease', 'L1', 'upgrade'],
    ['--lease', 'L1', 'tab-list'],
    ['--cdp', 'auto', '--lease', 'L1', 'snapshot'],
    ['snapshot'],
  ]) {
    refused(await run(args), 'E_UNSUPPORTED');
  }
  assert.deepEqual(calls, [
    ['request', { allow: [{ kind: 'origin', url: origin }], ttlSeconds: 3600, reason }],
    ['action', { lease: 'L1', op: 'goto', args: { url: origin } }],
    ...(leaseKernel
      ? []
      : [
          ['action', { lease: 'L1', op: 'click', args: { ref: 'e12' } }],
          ['action', { lease: 'L1', op: 'fill', args: { ref: 'e13', value: '--literal' } }],
        ]),
    ['action', { lease: 'L1', op: 'snapshot', args: {} }],
    ['action', { lease: 'L1', op: 'goto', args: { url: receipt } }],
    ['request', { lease: 'L1', allow: [{ kind: 'exact', url: receipt }], reason }],
    ['action', { lease: 'expired', op: 'snapshot', args: {} }],
  ]);
}
