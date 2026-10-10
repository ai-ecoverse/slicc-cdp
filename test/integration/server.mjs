import { createServer } from 'node:http';

const documents = new Set(['/', '/one', '/two', '/page', '/other']);

export function startServer() {
  const server = createServer((req, res) => {
    const url = new URL(req.url || '/', 'http://127.0.0.1');
    const chunks = [];
    req.on('data', (chunk) => chunks.push(chunk));
    req.on('end', () => answer(req, res, url.pathname, Buffer.concat(chunks)));
  });
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      const port = typeof address === 'object' && address ? address.port : 0;
      resolve({
        port,
        origin: `http://127.0.0.1:${port}`,
        close: () =>
          new Promise((done) => {
            server.closeAllConnections();
            server.close(() => done());
          }),
      });
    });
  });
}

function answer(req, res, pathname, body) {
  if (pathname === '/slow') return hold(req, res);
  if (pathname === '/api/me') return text(res, 200, req.headers.cookie || 'missing');
  if (pathname === '/json-post') return jsonPost(req, res, body);
  if (pathname === '/upload') return upload(res, body);
  if (pathname === '/show') return text(res, 200, 'hi', { 'x-test': 'yes' });
  if (pathname === '/only-head') return head(req, res);
  if (pathname === '/dump') return text(res, 200, 'body-text', { 'x-dump': '1' });
  if (pathname === '/code') return text(res, 201, '');
  if (pathname === '/nope') return text(res, 500, 'nope');
  if (pathname === '/child.html') return html(res, childPage());
  if (documents.has(pathname))
    return html(res, fixture(pathname === '/other' ? 'Other' : 'Fixture'));
  return text(res, 404, 'missing');
}

function hold(req, res) {
  const timer = setTimeout(() => {
    text(res, 200, 'late');
  }, 10000);
  req.on('close', () => clearTimeout(timer));
}

function jsonPost(req, res, body) {
  const ok = req.method === 'POST' && body.toString('utf8') === '{"a":1}';
  res.writeHead(ok ? 200 : 400, { 'content-type': 'application/json' });
  res.end(ok ? '{"ok":true}' : '{"ok":false}');
}

function upload(res, body) {
  const raw = body.toString('utf8');
  const ok = raw.includes('hello-bytes') && raw.includes('up.bin');
  text(res, ok ? 200 : 400, ok ? 'uploaded' : 'bad-upload');
}

function head(req, res) {
  if (req.method === 'HEAD') {
    res.writeHead(200, { 'content-type': 'text/plain' });
    res.end();
    return;
  }
  text(res, 200, 'SECRET');
}

function text(res, status, body, extra = {}) {
  res.writeHead(status, { 'content-type': 'text/plain', ...extra });
  res.end(body);
}

function html(res, body) {
  res.writeHead(200, {
    'content-type': 'text/html; charset=utf-8',
    'set-cookie': 'session=from-tab; Path=/; HttpOnly',
  });
  res.end(body);
}

function fixture(title) {
  return `<!doctype html><html><head><title>${title}</title></head><body><button id="go">Go</button><label for="name">Name</label><input id="name"><pre id="out"></pre><iframe src="/child.html"></iframe><script>document.getElementById('go').addEventListener('click',()=>{document.getElementById('out').textContent='clicked'});document.getElementById('name').addEventListener('keydown',(event)=>{if(event.key==='Enter'){document.getElementById('out').textContent='enter:'+event.target.value}});</script></body></html>`;
}

function childPage() {
  return '<!doctype html><title>Child</title><p>child-frame</p>';
}
