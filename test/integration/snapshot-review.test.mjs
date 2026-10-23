import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { promisify } from 'node:util';
import { fakeCdp } from './fake-cdp.mjs';
import { HARNESS_PORTS } from './ports.mjs';

const execute = promisify(execFile);
const root = path.resolve(import.meta.dirname, '../..');
const cli = path.join(
  root,
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);
const base = 'https://example.test/page';

async function snapshot(options, check) {
  const backend = await fakeCdp({ ...options, location: { href: base, title: 'Review' } });
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-snapshot-review-'));
  const env = {
    ...process.env,
    HOME: home,
    USERPROFILE: home,
    APPDATA: home,
    XDG_CONFIG_HOME: home,
    XDG_STATE_HOME: home,
    SLICC_CDP_PORTS: HARNESS_PORTS,
  };
  delete env.SLICC_CDP_URL;
  delete env.PLAYWRIGHT_CLI_SESSION;
  const run = (args) =>
    execute(cli, ['--cdp', backend.url, ...args], {
      cwd: home,
      env,
      timeout: 30000,
      maxBuffer: 1024 * 1024,
    });
  try {
    await run(['open', base]);
    const result = await run(['snapshot']);
    await check(result.stdout, backend, run);
  } finally {
    await backend.close();
    await rm(home, { recursive: true, force: true });
  }
}

function capture(json, refs = [], nodes = []) {
  return { json, refs, nodes, base };
}

function frame(id, url, securityOrigin) {
  return { frame: { id, loaderId: `L${id}`, url, securityOrigin } };
}

for (const metadata of [
  frame('PRIVATE', 'https://seven.sliccy.ai', 'https://seven.sliccy.ai'),
  frame('PRIVATE', 'about:blank', 'https://seven.sliccy.ai'),
  frame('PRIVATE', 'https://example.test/child', ''),
]) {
  test(`snapshot skips unreadable child metadata for ${metadata.frame.url}`, async () => {
    await snapshot(
      {
        frameTree: {
          ...frame('TAB1', base, 'https://example.test'),
          childFrames: [
            metadata,
            frame('SAFE', 'https://example.test/sibling', 'https://example.test'),
          ],
        },
        aria: (options) =>
          options.prefix === 'f1'
            ? capture([{ role: 'heading', name: 'CHILD_CONTENT_MUST_NOT_APPEAR' }])
            : options.prefix === 'f2'
              ? capture([{ role: 'heading', name: 'Allowed sibling' }])
              : capture(
                  [
                    { role: 'heading', name: 'Public heading' },
                    { role: 'iframe', ref: 'e1' },
                    { role: 'iframe', ref: 'e2' },
                  ],
                  [
                    { ref: 'e1', selector: 'iframe:nth-child(1)' },
                    { ref: 'e2', selector: 'iframe:nth-child(2)' },
                  ],
                  [
                    { backendNodeId: 1, frameId: 'PRIVATE' },
                    { backendNodeId: 2, frameId: 'SAFE' },
                  ]
                ),
      },
      async (output, backend) => {
        assert.match(output, /Public heading/);
        assert.match(output, /Allowed sibling/);
        assert.doesNotMatch(output, /CHILD_CONTENT_MUST_NOT_APPEAR/);
        assert.equal(
          backend.calls.some(
            (call) =>
              call.method === 'Page.createIsolatedWorld' && call.params?.frameId === 'PRIVATE'
          ),
          false
        );
      }
    );
  });
}

test('snapshot keeps the main tree when an ordinary child read is refused', async () => {
  await snapshot(
    {
      frameTree: {
        ...frame('TAB1', base, 'https://example.test'),
        childFrames: [frame('CHILD', 'https://example.test/child', 'https://example.test')],
      },
      aria: (options) =>
        options.prefix === 'f1'
          ? { error: 'slicc: refused: frame unavailable' }
          : capture(
              [
                { role: 'heading', name: 'Main remains available' },
                { role: 'iframe', ref: 'e1' },
              ],
              [{ ref: 'e1', selector: 'iframe' }],
              [{ backendNodeId: 1, frameId: 'CHILD' }]
            ),
    },
    async (output) => {
      assert.match(output, /Main remains available/);
    }
  );
});

test('snapshot resolves scheme-relative link credentials against its base before redaction', async () => {
  await snapshot(
    {
      aria: () =>
        capture([
          {
            role: 'link',
            name: 'External',
            url: '//user:SCHEME_USERINFO_VALUE@example.test/path?x=1#frag',
          },
        ]),
    },
    async (output) => {
      assert.match(output, /https:\/\/…:…@example\.test\/path\?x=…#…/);
      assert.doesNotMatch(output, /SCHEME_USERINFO_VALUE|user:/);
    }
  );
});

test('snapshot quotes YAML null and non-finite scalar strings', async () => {
  const scalars = ['~', '.inf', '.INF', '+.Inf', '-.inf', '.nan', '+.NaN', '-.NAN'];
  await snapshot(
    {
      aria: () =>
        capture([
          ...scalars.map((text) => ({ role: 'text', text })),
          { role: 'textbox', name: 'Input', placeholder: '~' },
        ]),
    },
    async (output) => {
      for (const scalar of scalars) assert.ok(output.includes(`- text: "${scalar}"`), scalar);
      assert.match(output, /- \/placeholder: "~"/);
    }
  );
});
