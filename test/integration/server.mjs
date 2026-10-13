import { createServer } from 'node:http';

const documents = new Set(['/', '/one', '/two', '/page', '/other']);
const hits = new Map();

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
  const query = new URL(req.url || '/', 'http://127.0.0.1').searchParams;
  if (pathname === '/authorize') return redirect(res, query.get('to') || '/');
  if (pathname === '/js-redirect') return html(res, jsRedirect(query.get('to') || '/'));
  hits.set(pathname, (hits.get(pathname) ?? 0) + 1);
  if (pathname === '/hits') return text(res, 200, String(hits.get(query.get('path')) ?? 0));
  if (pathname === '/api/leak')
    return text(res, 200, 'leaked', { 'access-control-allow-origin': '*' });
  if (pathname === '/landing' && query.get('leak')) return html(res, leakPage(query.get('leak')));
  if (pathname === '/cb' || pathname === '/landing') return html(res, fixture('Callback'));
  if (pathname === '/ping') return text(res, 200, 'pong', { 'access-control-allow-origin': '*' });
  if (pathname === '/blocked') return text(res, 200, 'not-blocked');
  if (pathname === '/api/headers') return headerEcho(req, res);
  if (pathname === '/slow') return hold(req, res);
  if (pathname === '/api/me') return text(res, 200, req.headers.cookie || 'missing');
  if (pathname === '/api/boot') return text(res, 200, 'booted');
  if (pathname === '/boot') return html(res, bootPage());
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

function redirect(res, to) {
  res.writeHead(302, { location: to, 'content-type': 'text/plain' });
  res.end('moved');
}

function leakPage(to) {
  return `<!doctype html><title>Leak</title><script>fetch(${JSON.stringify(to)}, { headers: { Authorization: 'Bearer LEAKKEY' } }).catch(() => {})</script>`;
}

function jsRedirect(to) {
  return `<!doctype html><title>Redirect</title><script>location.href=${JSON.stringify(to)}</script>`;
}

function headerEcho(req, res) {
  const added = req.headers['x-added'] || 'none';
  const dropped = req.headers['x-drop'] || 'none';
  text(res, 200, `x-added=${added} x-drop=${dropped}`);
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

function bootPage() {
  return '<!doctype html><title>Boot</title><script>fetch("/api/boot")</script>';
}
