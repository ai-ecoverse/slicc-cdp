import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import test from 'node:test';
import { fetchExtension } from './download.mjs';

const BYTES = Buffer.from('slicc-extension-zip-body');
const SHA = createHash('sha256').update(BYTES).digest('hex');

function listen(handler) {
  const server = createServer(handler);
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      resolve({
        port,
        origin: `http://127.0.0.1:${port}`,
        close: () =>
          new Promise((done) => {
            server.closeAllConnections?.();
            server.close(done);
          }),
      });
    });
  });
}

function extension(origin) {
  return {
    version: '9.9.9',
    url: `${origin}/releases/download/v9.9.9/slicc-extension.zip`,
    sha256: SHA,
  };
}

test('releases/download 504 twice then 200 returns the bytes', async () => {
  let hits = 0;
  const sleeps = [];
  const server = await listen((req, res) => {
    if (req.url !== '/releases/download/v9.9.9/slicc-extension.zip') {
      res.writeHead(404);
      res.end();
      return;
    }
    hits += 1;
    if (hits < 3) {
      res.writeHead(504);
      res.end('gateway');
      return;
    }
    res.writeHead(200, { 'content-type': 'application/zip' });
    res.end(BYTES);
  });
  try {
    const zip = await fetchExtension(extension(server.origin), {
      sleep: async (ms) => {
        sleeps.push(ms);
      },
    });
    assert.deepEqual(zip, BYTES);
    assert.equal(hits, 3);
    assert.deepEqual(sleeps, [1000, 2000]);
  } finally {
    await server.close();
  }
});

test('releases/download always 504 falls back to the API asset', async () => {
  let releaseHits = 0;
  const server = await listen((req, res) => {
    if (req.url === '/releases/download/v9.9.9/slicc-extension.zip') {
      releaseHits += 1;
      res.writeHead(504);
      res.end('gateway');
      return;
    }
    if (req.url === '/releases/tags/v9.9.9') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(
        JSON.stringify({
          assets: [{ name: 'slicc-extension.zip', url: `${server.origin}/releases/assets/1` }],
        })
      );
      return;
    }
    if (req.url === '/releases/assets/1') {
      assert.equal(req.headers.accept, 'application/octet-stream');
      res.writeHead(200, { 'content-type': 'application/octet-stream' });
      res.end(BYTES);
      return;
    }
    res.writeHead(404);
    res.end();
  });
  try {
    const zip = await fetchExtension(extension(server.origin), {
      sleep: async () => {},
      tagUrl: `${server.origin}/releases/tags/v9.9.9`,
    });
    assert.deepEqual(zip, BYTES);
    assert.equal(releaseHits, 4);
  } finally {
    await server.close();
  }
});

test('wrong sha256 is a hard failure with no further fallback', async () => {
  const server = await listen((req, res) => {
    res.writeHead(200);
    res.end(Buffer.from('wrong-bytes'));
  });
  try {
    await assert.rejects(
      () =>
        fetchExtension(extension(server.origin), {
          sleep: async () => {},
        }),
      (error) => {
        assert.match(
          error.message,
          new RegExp(
            `${extension(server.origin).url.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}: sha256 [a-f0-9]{64}, want ${SHA}`
          )
        );
        return true;
      }
    );
  } finally {
    await server.close();
  }
});

test('releases/download and API asset both failing name both URL classes', async () => {
  const server = await listen((_req, res) => {
    res.writeHead(504);
    res.end('gateway');
  });
  try {
    await assert.rejects(
      () =>
        fetchExtension(extension(server.origin), {
          sleep: async () => {},
          tagUrl: `${server.origin}/releases/tags/v9.9.9`,
        }),
      (error) => {
        assert.match(error.message, /releases\/download failed \(.*HTTP 504/);
        assert.match(error.message, /api asset failed \(.*HTTP 504/);
        return true;
      }
    );
  } finally {
    await server.close();
  }
});

test('Retry-After is honoured before the next attempt', async () => {
  let hits = 0;
  const sleeps = [];
  const server = await listen((req, res) => {
    hits += 1;
    if (hits === 1) {
      res.writeHead(429, { 'retry-after': '2' });
      res.end('slow down');
      return;
    }
    res.writeHead(200);
    res.end(BYTES);
  });
  try {
    const zip = await fetchExtension(extension(server.origin), {
      sleep: async (ms) => {
        sleeps.push(ms);
      },
    });
    assert.deepEqual(zip, BYTES);
    assert.deepEqual(sleeps, [2000]);
  } finally {
    await server.close();
  }
});

test('HTTP 404 is not retried before the API path', async () => {
  let releaseHits = 0;
  const sleeps = [];
  const server = await listen((req, res) => {
    if (req.url === '/releases/download/v9.9.9/slicc-extension.zip') {
      releaseHits += 1;
      res.writeHead(404);
      res.end('missing');
      return;
    }
    if (req.url === '/releases/tags/v9.9.9') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(
        JSON.stringify({
          assets: [{ name: 'slicc-extension.zip', url: `${server.origin}/releases/assets/1` }],
        })
      );
      return;
    }
    if (req.url === '/releases/assets/1') {
      res.writeHead(200);
      res.end(BYTES);
      return;
    }
    res.writeHead(404);
    res.end();
  });
  try {
    const zip = await fetchExtension(extension(server.origin), {
      sleep: async (ms) => {
        sleeps.push(ms);
      },
      tagUrl: `${server.origin}/releases/tags/v9.9.9`,
    });
    assert.deepEqual(zip, BYTES);
    assert.equal(releaseHits, 1);
    assert.deepEqual(sleeps, []);
  } finally {
    await server.close();
  }
});
