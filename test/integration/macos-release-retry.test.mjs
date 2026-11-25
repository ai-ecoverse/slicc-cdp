import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const scripts = fileURLToPath(new URL('../../scripts/', import.meta.url));
const signed =
  'Executable=bin\nIdentifier=ai.sliccy.curlwright\nAuthority=Developer ID Application: Example (TEAM123)\nAuthority=Developer ID Certification Authority\nAuthority=Apple Root CA\n';
const development =
  'Executable=bin\nIdentifier=ai.sliccy.curlwright\nAuthority=Apple Development: Example (TEAM123)\nAuthority=Apple Worldwide Developer Relations Certification Authority\n';
const notarised = (bin) =>
  `${bin}: accepted\nsource=Notarized Developer ID\norigin=Developer ID Application: Example (TEAM123)\n`;
const pending = (bin, n) =>
  `${bin}: rejected pending-${n}\nsource=Unnotarized Developer ID\norigin=Developer ID Application: Example (TEAM123)\n`;

function sandbox(t, plan) {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cdp-macos-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  mkdirSync(path.join(dir, 'bin'));
  writeFileSync(path.join(dir, 'plan.json'), JSON.stringify(plan));
  const fake = (name, body) => {
    writeFileSync(path.join(dir, 'bin', name), `#!${process.execPath}\n${body}`);
    chmodSync(path.join(dir, 'bin', name), 0o755);
  };
  const step = `
const fs = require('node:fs');
const dir = ${JSON.stringify(dir)};
const args = process.argv.slice(2);
const step = (name) => {
  const log = dir + '/' + name + '.log';
  const n = fs.existsSync(log) ? fs.readFileSync(log, 'utf8').split('\\n').filter(Boolean).length : 0;
  fs.appendFileSync(log, args.join(' ') + '\\n');
  const entries = JSON.parse(fs.readFileSync(dir + '/plan.json', 'utf8'))[name] ?? [];
  return entries[Math.min(n, entries.length - 1)] ?? { code: 0 };
};
const reply = (entry) => {
  if (entry.stdout) process.stdout.write(entry.stdout);
  if (entry.stderr) process.stderr.write(entry.stderr);
  process.exit(entry.code ?? 0);
};
`;
  fake('uname', `${step}process.stdout.write('Darwin\\n');`);
  fake('zip', `${step}step('zip');`);
  fake('openssl', `${step}if (args[0] === 'rand') process.stdout.write('cGFzc3dvcmQ=\\n');`);
  fake(
    'security',
    `${step}if (args[0] === 'find-identity') process.stdout.write('  1) ABCDEF0123 "Developer ID Application: Example (TEAM123)"\\n     1 valid identities found\\n');
if (args[0] === 'list-keychains' && !args.includes('-s')) process.stdout.write('    "/Users/runner/Library/Keychains/login.keychain-db"\\n');`
  );
  fake(
    'codesign',
    `${step}if (args[0] === '-dv') reply(step('codesign')); else fs.appendFileSync(dir + '/sign.log', args.join(' ') + '\\n');`
  );
  fake('spctl', `${step}reply(step('spctl'));`);
  fake(
    'xcrun',
    `${step}if (args[1] === 'submit') reply(step('submit'));
if (args[1] === 'log') { fs.appendFileSync(dir + '/notarylog.log', args[2] + '\\n'); process.stdout.write('notary log for ' + args[2] + '\\n'); }`
  );
  fake('sleep', `${step}step('sleep');`);
  const env = {
    ...process.env,
    PATH: `${path.join(dir, 'bin')}:${process.env.PATH}`,
    RUNNER_TEMP: dir,
    APPLE_CERTIFICATE_BASE64: 'Y2VydA==',
    APPLE_CERTIFICATE_PASSWORD: 'certificate-password',
    APPLE_API_KEY_P8_BASE64: 'a2V5',
    APPLE_API_KEY_ID: 'KEY123',
    APPLE_API_KEY_ISSUER_ID: 'issuer-123',
  };
  const lines = (name) => {
    try {
      return readFileSync(path.join(dir, `${name}.log`), 'utf8')
        .split('\n')
        .filter(Boolean);
    } catch {
      return [];
    }
  };
  const run = (script, args) => {
    const result = spawnSync('bash', [path.join(scripts, script), ...args], {
      cwd: dir,
      encoding: 'utf8',
      env,
    });
    return { ...result, lines };
  };
  return run;
}

const submission = (status, id) => ({
  stdout: `${JSON.stringify({ id, status, message: status })}\n`,
});
const lost = {
  code: 1,
  stderr:
    'Error: HTTPError(statusCode: nil, error: Error Domain=NSURLErrorDomain Code=-1001 "The request timed out.")\n',
};

function notarise(t, submits) {
  const run = sandbox(t, { submit: submits });
  return run('sign-macos.sh', ['ai.sliccy.playwright-cli:pw', 'ai.sliccy.curlwright:cw']);
}

test('notarisation accepted on the first attempt signs, submits once and does not wait', (t) => {
  const result = notarise(t, [submission('Accepted', 'sub-1')]);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.equal(result.lines('submit').length, 1);
  assert.deepEqual(result.lines('sleep'), []);
  assert.equal(result.lines('sign').filter((line) => line.startsWith('--force')).length, 2);
  assert.match(result.stdout, /^notarization attempt 1\/3: submission sub-1, status Accepted$/m);
  assert.match(result.stdout, /^notarization accepted \(sub-1\)$/m);
});

test('a lost or timed-out notarisation request is submitted again after 30 s and passes when accepted', (t) => {
  const result = notarise(t, [
    lost,
    submission('In Progress', 'sub-2'),
    submission('Accepted', 'sub-3'),
  ]);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.equal(result.lines('submit').length, 3);
  assert.deepEqual(result.lines('sleep'), ['30', '90']);
  assert.match(result.stdout, /^notarization attempt 1\/3: submission unknown, status unknown$/m);
  assert.match(result.stdout, /^notarization attempt 2\/3: submission sub-2, status In Progress$/m);
  assert.match(result.stdout, /^notarization attempt 3\/3: submission sub-3, status Accepted$/m);
  assert.match(result.stdout, /^notarization accepted \(sub-3\)$/m);
});

test('a notarisation request accepted on the second attempt waits only 30 s', (t) => {
  const result = notarise(t, [lost, submission('Accepted', 'sub-4')]);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.equal(result.lines('submit').length, 2);
  assert.deepEqual(result.lines('sleep'), ['30']);
  assert.match(result.stdout, /^notarization accepted \(sub-4\)$/m);
});

test('an Invalid notarisation fails at once with the error and the notary log', (t) => {
  const result = notarise(t, [submission('Invalid', 'sub-bad'), submission('Accepted', 'sub-ok')]);
  assert.equal(result.status, 1);
  assert.equal(result.lines('submit').length, 1, 'a rejection is never submitted again');
  assert.deepEqual(result.lines('sleep'), []);
  assert.match(result.stdout, /^notarization attempt 1\/3: submission sub-bad, status Invalid$/m);
  assert.match(result.stdout, /^::error::notarization ended with status Invalid$/m);
  assert.match(result.stderr, /"status":"Invalid"/);
  assert.deepEqual(result.lines('notarylog'), ['sub-bad']);
  assert.match(result.stderr, /^notary log for sub-bad$/m);
  assert.doesNotMatch(result.stdout, /notarization accepted/);
});

test('a Rejected notarisation fails at once with the error and the notary log', (t) => {
  const result = notarise(t, [submission('Rejected', 'sub-no'), submission('Accepted', 'sub-ok')]);
  assert.equal(result.status, 1);
  assert.equal(result.lines('submit').length, 1, 'a rejection is never submitted again');
  assert.deepEqual(result.lines('sleep'), []);
  assert.match(result.stdout, /^notarization attempt 1\/3: submission sub-no, status Rejected$/m);
  assert.match(result.stdout, /^::error::notarization ended with status Rejected$/m);
  assert.match(result.stderr, /"status":"Rejected"/);
  assert.deepEqual(result.lines('notarylog'), ['sub-no']);
  assert.match(result.stderr, /^notary log for sub-no$/m);
  assert.doesNotMatch(result.stdout, /notarization accepted/);
});

test('a notarisation that never reports a status fails after three attempts', (t) => {
  const result = notarise(t, [lost]);
  assert.equal(result.status, 1);
  assert.equal(result.lines('submit').length, 3);
  assert.deepEqual(result.lines('sleep'), ['30', '90']);
  for (const n of [1, 2, 3])
    assert.match(
      result.stdout,
      new RegExp(`^notarization attempt ${n}/3: submission unknown, status unknown$`, 'm')
    );
  assert.match(result.stdout, /^::error::notarization ended with status unknown$/m);
  assert.deepEqual(result.lines('notarylog'), []);
  assert.doesNotMatch(result.stdout, /notarization accepted/);
});

function gatekeeper(t, plan, bins = ['dist/curlwright-darwin-arm64']) {
  const run = sandbox(t, plan);
  return run('check-gatekeeper.sh', bins);
}

test('Gatekeeper accepting every binary on the first attempt passes without waiting', (t) => {
  const bins = ['dist/playwright-cli-darwin-arm64', 'dist/curlwright-darwin-arm64'];
  const result = gatekeeper(
    t,
    {
      codesign: [{ stderr: signed }],
      spctl: [{ stderr: notarised(bins[0]) }, { stderr: notarised(bins[1]) }],
    },
    bins
  );
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.equal(result.lines('spctl').length, 2);
  assert.deepEqual(result.lines('sleep'), []);
  for (const bin of bins) {
    assert.ok(result.stdout.includes(`${bin}: accepted\nsource=Notarized Developer ID\n`), bin);
    assert.ok(
      result.stdout.includes(`gatekeeper accepted ${bin} as notarised on attempt 1/3`),
      bin
    );
  }
});

test('Gatekeeper rejecting a binary whose ticket is not yet visible is checked again and passes', (t) => {
  const bin = 'dist/curlwright-darwin-arm64';
  const result = gatekeeper(t, {
    codesign: [{ stderr: signed }],
    spctl: [{ code: 3, stderr: pending(bin, 1) }, { stderr: notarised(bin) }],
  });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.equal(result.lines('spctl').length, 2);
  assert.deepEqual(result.lines('sleep'), ['20']);
  assert.ok(result.stdout.includes(pending(bin, 1)), 'the rejected output is printed');
  assert.match(result.stdout, /^codesign exit 0, spctl exit 3$/m);
  assert.ok(result.stdout.includes(`gatekeeper accepted ${bin} as notarised on attempt 2/3`));
});

test('Gatekeeper rejecting a binary three times fails with every attempt in the log', (t) => {
  const bin = 'dist/curlwright-darwin-arm64';
  const result = gatekeeper(t, {
    codesign: [{ stderr: signed }],
    spctl: [
      { code: 3, stderr: pending(bin, 1) },
      { code: 3, stderr: pending(bin, 2) },
      { code: 3, stderr: pending(bin, 3) },
      { stderr: notarised(bin) },
    ],
  });
  assert.equal(result.status, 1);
  assert.equal(result.lines('spctl').length, 3);
  assert.deepEqual(result.lines('sleep'), ['20', '60']);
  let at = 0;
  for (const n of [1, 2, 3]) {
    const next = result.stdout.indexOf(pending(bin, n), at);
    assert.ok(next >= at, `attempt ${n} output is printed in order`);
    at = next;
    assert.ok(result.stdout.includes(`gatekeeper attempt ${n}/3 for ${bin}`));
  }
  const error = result.stdout.indexOf(
    `::error::Gatekeeper did not accept ${bin} as notarised after 3 attempts`
  );
  assert.ok(error > at, 'the failure follows the printed outputs');
  assert.doesNotMatch(result.stdout, /gatekeeper accepted/);
});

test('Gatekeeper accepting a binary without a Developer ID Application authority fails at once', (t) => {
  const bin = 'dist/curlwright-darwin-arm64';
  const result = gatekeeper(t, {
    codesign: [{ stderr: development }, { stderr: signed }],
    spctl: [{ stderr: notarised(bin) }],
  });
  assert.equal(result.status, 1);
  assert.equal(result.lines('spctl').length, 1, 'a wrong authority is never retried');
  assert.equal(result.lines('codesign').length, 1);
  assert.deepEqual(result.lines('sleep'), []);
  assert.ok(result.stdout.includes(development), 'the codesign output is printed');
  assert.ok(result.stdout.includes(notarised(bin)), 'the spctl output is printed');
  assert.ok(
    result.stdout.includes(
      `::error::${bin} is not signed by a Developer ID Application certificate`
    )
  );
  assert.doesNotMatch(result.stdout, /gatekeeper accepted/);
});

test('a binary whose signature cannot be read fails at once', (t) => {
  const bin = 'dist/curlwright-darwin-arm64';
  const result = gatekeeper(t, {
    codesign: [
      { code: 1, stderr: `${bin}: code object is not signed at all\n` },
      { stderr: signed },
    ],
    spctl: [{ code: 3, stderr: `${bin}: rejected\n` }, { stderr: notarised(bin) }],
  });
  assert.equal(result.status, 1);
  assert.equal(result.lines('codesign').length, 1);
  assert.equal(result.lines('spctl').length, 1);
  assert.deepEqual(result.lines('sleep'), []);
  assert.ok(result.stdout.includes(`${bin}: code object is not signed at all`));
  assert.ok(result.stdout.includes(`::error::codesign could not read the signature of ${bin}`));
});
