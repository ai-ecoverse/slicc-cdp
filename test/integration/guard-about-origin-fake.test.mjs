import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import './watchdog.mjs';

const exe = (name) =>
  path.resolve('target/debug', process.platform === 'win32' ? `${name}.exe` : name);
const wasm = (name) => path.resolve('target/wasm32-wasip1/debug', `${name}.wasm`);
const runners = {
  native: async (url) => nativeRunner(exe('playwright-cli'), exe('curlwright'), url),
  wasi: async (url) => kernelRunner(wasm('playwright-cli'), wasm('curlwright'), url),
};

async function withBrowser(runner, options, body) {
  const fake = await fakeCdp(options);
  const pw = await runner(fake.url);
  try {
    const cwd = await pw.fresh();
    await body((args) => pw.playwright(args, cwd), fake);
  } finally {
    await pw.close();
    await fake.close();
  }
}

for (const [transport, runner] of Object.entries(runners)) {
  test(`${transport}: a blank tab whose context origin reads about:// opens and navigates`, async () => {
    await withBrowser(runner, { blankOrigin: 'about://' }, async (run) => {
      const opened = await run(['open', 'http://example.test/page']);
      assert.equal(opened.code, 0, opened.stderr);
      assert.match(opened.stdout, /Opened http:\/\/example\.test\/page in new tab/);
      const title = await run(['eval', 'location.href']);
      assert.equal(title.code, 0, title.stderr);
    });
  });

  test(`${transport}: a blank tab whose context origin reads about:// still refuses seven and the vault`, async () => {
    await withBrowser(runner, { blankOrigin: 'about://' }, async (run) => {
      for (const url of ['https://seven.sliccy.ai/', 'https://vault.sliccy.ai/']) {
        const opened = await run(['open', url]);
        assert.equal(opened.code, 1, url);
        assert.match(opened.stderr, /slicc: refused/, url);
      }
    });
  });

  test(`${transport}: a blank tab opened as about:blank with an about:// context origin answers eval`, async () => {
    await withBrowser(runner, { blankOrigin: 'about://' }, async (run) => {
      const opened = await run(['open', 'about:blank']);
      assert.equal(opened.code, 0, opened.stderr);
      const evaluated = await run(['eval', '1']);
      assert.equal(evaluated.code, 0, evaluated.stderr);
    });
  });
  test(`${transport}: an opaque child frame still refuses its page when contexts read about://`, async () => {
    const sandbox = {
      id: 'SANDBOX',
      parentId: 'TAB',
      loaderId: 'SL',
      url: 'about:blank',
      securityOrigin: '://',
    };
    await withBrowser(runner, { blankOrigin: 'about://' }, async (run, fake) => {
      fake.targets.push({
        targetId: 'TAB',
        url: 'http://example.test/',
        childFrames: [{ frame: sandbox }],
      });
      for (const args of [['eval', '1'], ['snapshot']]) {
        const result = await run(args);
        assert.equal(result.code, 1, args[0]);
        assert.match(result.stderr, /slicc: refused/, args[0]);
      }
    });
  });
}

const SEVEN = 'https://seven.sliccy.ai/';
const child = (id, parentId, url, securityOrigin) => ({
  frame: { id, parentId, loaderId: `${id}-L`, url, securityOrigin },
});
const twins = {
  'a blank tab slicc did not open': [{ targetId: 'TAB', url: 'about:blank' }],
  'a blank frame under seven': [
    { targetId: 'TAB', url: SEVEN, childFrames: [child('C', 'TAB', 'about:blank', '://')] },
  ],
  'a srcdoc frame inheriting seven': [
    {
      targetId: 'TAB',
      url: SEVEN,
      childFrames: [child('C', 'TAB', 'about:srcdoc', 'https://seven.sliccy.ai')],
    },
  ],
  'a sandboxed data frame of seven': [
    { targetId: 'TAB', url: SEVEN, childFrames: [child('C', 'TAB', 'data:text/html,x', '://')] },
  ],
  'a blank popup of seven': [
    { targetId: 'TAB', url: 'about:blank', openerId: 'OPENER' },
    { targetId: 'OPENER', url: SEVEN },
  ],
  'a blank frame under an unplaced blank frame': [
    {
      targetId: 'TAB',
      url: 'http://example.test/',
      childFrames: [
        {
          ...child('P', 'TAB', 'about:blank', '://'),
          childFrames: [child('C', 'P', 'about:blank', '://')],
        },
      ],
    },
  ],
};

for (const [transport, runner] of Object.entries(runners)) {
  for (const [name, targets] of Object.entries(twins)) {
    test(`${transport}: ${name} stays refused whether contexts read :// or about://`, async () => {
      for (const blankOrigin of ['://', 'about://']) {
        await withBrowser(runner, { blankOrigin }, async (run, fake) => {
          fake.targets.push(...targets);
          for (const args of [
            ['eval', '1', '--tab', 'TAB'],
            ['snapshot', '--tab', 'TAB'],
          ]) {
            const result = await run(args);
            assert.equal(result.code, 1, `${blankOrigin} ${args[0]}`);
            assert.match(result.stderr, /slicc: refused/, `${blankOrigin} ${args[0]}`);
          }
        });
      }
    });
  }

  test(`${transport}: a later protected context for the same blank frame still marks the tab protected`, async () => {
    for (const blankOrigin of ['://', 'about://']) {
      const late = (targets) => (message) => {
        if (message.method !== 'Runtime.enable') return;
        for (const target of targets())
          if (target.url === 'about:blank' && !target.worlds)
            target.worlds = [
              {
                id: 2,
                uniqueId: `late:${target.targetId}`,
                origin: 'https://seven.sliccy.ai',
                name: '',
                auxData: { frameId: target.targetId, isDefault: true },
              },
            ];
      };
      let fake;
      fake = await fakeCdp({ blankOrigin, onCommand: late(() => fake.targets) });
      const pw = await runner(fake.url);
      try {
        const cwd = await pw.fresh();
        const opened = await pw.playwright(['open', 'http://example.test/page'], cwd);
        assert.equal(opened.code, 1, blankOrigin);
        assert.match(opened.stderr, /slicc: refused/, blankOrigin);
        assert.ok(
          !fake.calls.some((call) => call.method === 'Page.navigate'),
          `${blankOrigin} navigated`
        );
      } finally {
        await pw.close();
        await fake.close();
      }
    }
  });

  test(`${transport}: a context origin of about://settings is not read as a blank tab`, async () => {
    await withBrowser(runner, { blankOrigin: 'about://settings' }, async (run) => {
      const opened = await run(['open', 'http://example.test/page']);
      assert.equal(opened.code, 1);
      assert.match(opened.stderr, /slicc: refused/);
    });
  });

  test(`${transport}: an isolated world on a blank tab whose origin reads about:// is not admitted through the mapping`, async () => {
    await withBrowser(runner, { blankOrigin: 'about://' }, async (run, fake) => {
      assert.equal((await run(['open', 'about:blank'])).code, 0);
      const snapshot = await run(['snapshot']);
      assert.equal(snapshot.code, 1);
      assert.match(snapshot.stderr, /slicc: refused/);
      assert.ok(fake.calls.some((call) => call.method === 'Page.createIsolatedWorld'));
      assert.equal((await run(['eval', '1'])).code, 0);
    });
  });

  test(`${transport}: seven and the vault open together stay refused under both context origins`, async () => {
    for (const blankOrigin of ['://', 'about://']) {
      await withBrowser(runner, { blankOrigin }, async (run, fake) => {
        fake.targets.push(
          { targetId: 'SEVEN', url: SEVEN },
          { targetId: 'VAULT', url: 'https://vault.sliccy.ai/' }
        );
        for (const tab of ['SEVEN', 'VAULT']) {
          const result = await run(['eval', '1', '--tab', tab]);
          assert.equal(result.code, 1, `${blankOrigin} ${tab}`);
          assert.match(result.stderr, /slicc: refused/, `${blankOrigin} ${tab}`);
        }
      });
    }
  });
}

const refused = (result, label) => {
  assert.equal(result.code, 1, `${label}: ${result.stdout}${result.stderr}`);
  assert.match(result.stderr, /slicc: refused/, label);
};
const admitted = (result, label) => assert.equal(result.code, 0, `${label}: ${result.stderr}`);
const STEPS = [['eval', '1'], ['snapshot'], ['goto', 'http://example.test/next']];

for (const [transport, runner] of Object.entries(runners)) {
  for (const blankOrigin of ['ABOUT://', 'about://x', 'about:blank', 'ABOUT:BLANK']) {
    test(`${transport}: a created blank tab whose default context origin reads ${blankOrigin} is refused`, async () => {
      await withBrowser(runner, { blankOrigin }, async (run, fake) => {
        refused(await run(['open', 'http://example.test/page']), `${blankOrigin} open`);
        assert.ok(
          !fake.calls.some((call) => call.method === 'Page.navigate'),
          `${blankOrigin} navigated`
        );
        for (const args of STEPS) refused(await run(args), `${blankOrigin} ${args[0]}`);
      });
    });
  }

  for (const blankOrigin of ['://', 'about://', 'https://example.test', 'xabout:blank']) {
    test(`${transport}: a created blank tab whose default context origin reads ${blankOrigin} is admitted`, async () => {
      await withBrowser(runner, { blankOrigin }, async (run) => {
        admitted(await run(['open', 'http://example.test/page']), `${blankOrigin} open`);
        for (const args of STEPS) admitted(await run(args), `${blankOrigin} ${args[0]}`);
      });
    });
  }
}

const contexts = {
  'an isolated world': (frameId) => ({ frameId, isDefault: false }),
  'a frameless context': () => ({ isDefault: true }),
  'a context with an empty frameId': () => ({ isDefault: true, frameId: '' }),
  'a context whose isDefault is not a boolean': (frameId) => ({ frameId, isDefault: 'true' }),
};
const kept = {
  'about:blank': () => [0, 0, 0, 0],
  'ABOUT://': (name) => (name === 'an isolated world' ? [0, 0, 1, 0] : [1, 1, 1, 1]),
};

for (const [transport, runner] of Object.entries(runners)) {
  for (const [name, auxData] of Object.entries(contexts)) {
    for (const [origin, expected] of Object.entries(kept)) {
      test(`${transport}: ${name} reading ${origin} on a created blank tab answers as before the about: rule`, async () => {
        let fake;
        fake = await fakeCdp({
          onCommand: (message) => {
            if (message.method !== 'Runtime.enable') return;
            for (const target of fake.targets)
              if (!target.worlds)
                target.worlds = [
                  {
                    id: 3,
                    uniqueId: `extra:${target.targetId}`,
                    origin,
                    name: 'extra',
                    auxData: auxData(target.targetId),
                  },
                ];
          },
        });
        const pw = await runner(fake.url);
        try {
          const cwd = await pw.fresh();
          const codes = [];
          for (const args of [['open', 'http://example.test/page'], ...STEPS]) {
            const result = await pw.playwright(args, cwd);
            codes.push(result.code);
            if (result.code !== 0) assert.match(result.stderr, /slicc: refused/, args[0]);
          }
          assert.deepEqual(codes, expected(name), `${name} ${origin}`);
        } finally {
          await pw.close();
          await fake.close();
        }
      });
    }
  }
}
