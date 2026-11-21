import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { cdpProxy } from './cdp-proxy.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { startServer } from './server.mjs';
import { killTree } from './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

test('the cdp-client Fetch API against chromium', async () => {
  const site = await startServer();
  let browser;
  try {
    browser = await launchBrowser([
      `--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1:${site.port}, MAP vault.sliccy.ai 127.0.0.1:${site.port}`,
      '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
    ]);
    const result = await cargo(
      [
        'test',
        '-p',
        'slicc-cdp-client',
        '--lib',
        '--',
        '--ignored',
        '--exact',
        'fetch::tests::live_chromium_interception',
        'shield::tests::live_protected_targets',
        '--test-threads=1',
      ],
      {
        SLICC_FETCH_LIVE_WS: browser.wsUrl,
        SLICC_FETCH_LIVE_APP: site.origin,
        SLICC_FETCH_LIVE_IDP: `http://localhost:${site.port}`,
        SLICC_FETCH_LIVE_SEVEN: 'http://seven.sliccy.ai',
        SLICC_FETCH_LIVE_VAULT: 'http://vault.sliccy.ai',
      }
    );
    assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
    assert.match(result.stdout, /2 passed/);
  } finally {
    await browser?.close();
    await site.close();
  }
});

test('interception that turns active after Fetch.enable answers is waited for', async () => {
  await throughProxy({ early: [{ method: 'Fetch.enable', match: '/api/headers', ms: 1500 }] });
});

test('interception waits until the browser has committed a navigation the renderer already reported', async () => {
  await throughProxy({
    before: [
      {
        method: 'Page.navigate',
        match: '/js-redirect',
        send: { method: 'Emulation.setCPUThrottlingRate', params: { rate: 50 } },
      },
    ],
  });
});

test('pending-commit attach refuses without proof and without latching seven', {
  timeout: 180_000,
}, async (t) => {
  const proxy = await throughProxy(
    {
      before: [
        {
          method: 'Fetch.enable',
          match: '"resourceType":"Document"',
          send: { method: 'Page.getNavigationHistory' },
        },
      ],
    },
    'shield::tests::live_pending_commit_attach_refuses_without_proof_or_latch',
    t
  );
  assert.equal(
    proxy.evidence.includes('(a) protected fixture hit or commit: guard bypass'),
    false,
    proxy.evidence
  );
  assert.match(
    proxy.evidence,
    /attach_outcome=(no-proof-unverified|unanswered-timeout|admitted)/,
    proxy.evidence
  );
});

async function fixtureHits(site) {
  return Object.fromEntries(
    await Promise.all(
      ['/code', '/dump'].map(async (path) => {
        const response = await fetch(`${site.origin}/hits?path=${path}`);
        assert(response.ok, `fixture hits unavailable: ${response.status}`);
        return [path, Number(await response.text())];
      })
    )
  );
}

function sevenCommits(events) {
  return events.filter(
    ({ method, params }) =>
      (method === 'Page.frameNavigated' &&
        params.frame?.url?.startsWith('http://seven.sliccy.ai/')) ||
      (method === 'Network.responseReceived' &&
        params.response?.url?.startsWith('http://seven.sliccy.ai/'))
  );
}

async function throughProxy(options, name = 'fetch::tests::live_chromium_interception', t) {
  const guardRace =
    name === 'shield::tests::live_pending_commit_attach_refuses_without_proof_or_latch';
  const site = await startServer();
  const initialHits = await fixtureHits(site);
  let browser;
  let proxy;
  let guardEvidence;
  try {
    browser = await launchBrowser([
      `--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1:${site.port}, MAP vault.sliccy.ai 127.0.0.1:${site.port}`,
      '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
    ]);
    const events = [];
    const transform = options.transform ?? ((message) => message);
    proxy = await cdpProxy(browser.wsUrl, {
      ...options,
      transform: (message) => {
        if (message.method) events.push(message);
        return transform(message);
      },
    });
    let guardEvidenceUrl;
    if (guardRace) {
      guardEvidence = createServer((_req, res) => {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(
          JSON.stringify({
            window_replies: proxy.replies,
            seven_commits: sevenCommits(events),
          })
        );
      });
      await new Promise((resolve) => guardEvidence.listen(0, '127.0.0.1', resolve));
      guardEvidenceUrl = `http://127.0.0.1:${guardEvidence.address().port}/`;
    }
    const result = await cargo(
      [
        'test',
        '-p',
        'slicc-cdp-client',
        '--lib',
        '--',
        '--ignored',
        '--exact',
        name,
        '--test-threads=1',
        ...(guardRace ? ['--nocapture'] : []),
      ],
      {
        SLICC_FETCH_LIVE_WS: proxy.url,
        ...(guardEvidenceUrl ? { SLICC_FETCH_LIVE_GUARD_EVIDENCE: guardEvidenceUrl } : {}),
        SLICC_FETCH_LIVE_APP: site.origin,
        SLICC_FETCH_LIVE_IDP: `http://localhost:${site.port}`,
        SLICC_FETCH_LIVE_SEVEN: 'http://seven.sliccy.ai',
        SLICC_FETCH_LIVE_VAULT: 'http://vault.sliccy.ai',
      },
      guardRace ? 120_000 : 600_000
    );
    const hits = await fixtureHits(site);
    const delta = Object.fromEntries(
      Object.entries(hits).map(([path, count]) => [path, count - initialHits[path]])
    );
    const commits = sevenCommits(events);
    const cargoOutput = `${result.stdout}\n${result.stderr}`;
    const classification = !guardRace
      ? 'interception diagnostics'
      : Object.values(delta).some((count) => count > 0) || commits.length
        ? '(a) protected fixture hit or commit: guard bypass'
        : 'no protected hit or commit observed; inspect timer and pending-window evidence';
    const pendingWindow = !guardRace
      ? 'not applicable'
      : /attach_outcome=admitted/.test(cargoOutput)
        ? 'admitted-window'
        : /attach_outcome=(no-proof-unverified|unanswered-timeout)/.test(cargoOutput)
          ? 'no-proof-refusal'
          : 'unknown';
    proxy.evidence = `${classification}\npending_window=${pendingWindow}\nfixture_hits=${JSON.stringify({ initialHits, hits, delta })}\nseven_commits=${JSON.stringify(commits)}\nwindow_replies=${JSON.stringify(proxy.replies)}\nbrowser_events=${JSON.stringify(events)}\nstdout=${result.stdout}\nstderr=${result.stderr}`;
    assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}\n${proxy.evidence}`);
    if (guardRace) {
      assert.equal(commits.length, 0, proxy.evidence);
      assert(
        Object.values(delta).every((count) => count === 0),
        proxy.evidence
      );
      const counts = cargoOutput.match(
        /attempt_counts not_attempted=\d+ no_proof=\d+ admitted=\d+/
      );
      assert.match(
        cargoOutput,
        /attempt_counts not_attempted=\d+ no_proof=\d+ admitted=\d+/,
        proxy.evidence
      );
      if (counts) {
        if (t) t.diagnostic(counts[0]);
        else console.log(counts[0]);
      }
    }
    assert.match(result.stdout, /1 passed/);
    return proxy;
  } finally {
    if (guardEvidence) {
      guardEvidence.closeAllConnections();
      await new Promise((resolve) => guardEvidence.close(resolve));
    }
    await proxy?.close();
    await browser?.close();
    await site.close();
  }
}

function cargo(args, extra, timeoutMs = 600_000) {
  return new Promise((resolve, reject) => {
    const env = { ...process.env, ...extra };
    delete env.SLICC_CDP_URL;
    const child = spawn('cargo', args, {
      cwd: root,
      env,
      detached: process.platform !== 'win32',
    });
    const out = [];
    const err = [];
    child.stdout.on('data', (chunk) => out.push(chunk));
    child.stderr.on('data', (chunk) => err.push(chunk));
    const timer = setTimeout(() => {
      killTree(child);
      reject(new Error(`cargo test timed out after ${timeoutMs}ms`));
    }, timeoutMs);
    child.on('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on('close', (code) => {
      clearTimeout(timer);
      resolve({
        code: code ?? 1,
        stdout: Buffer.concat(out).toString('utf8'),
        stderr: Buffer.concat(err).toString('utf8'),
      });
    });
  });
}
