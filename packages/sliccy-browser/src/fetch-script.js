function bytesToBase64(bytes) {
  let binary = '';
  const chunkSize = 0x8000;
  for (let i = 0; i < bytes.length; i += chunkSize) {
    binary += String.fromCharCode(...Array.from(bytes.subarray(i, i + chunkSize)));
  }
  return btoa(binary);
}

async function serializeFormData(form) {
  const entries = [];
  for (const [name, value] of form.entries()) {
    if (typeof value === 'string') {
      entries.push({ name, value });
      continue;
    }
    const bytes = new Uint8Array(await value.arrayBuffer());
    entries.push({
      name,
      file: {
        data: bytesToBase64(bytes),
        filename: typeof value.name === 'string' ? value.name : 'blob',
        type: value.type || '',
      },
    });
  }
  return { kind: 'formdata', entries };
}

async function serializeBody(raw, headers) {
  const hasContentType = () =>
    Object.keys(headers).some((key) => key.toLowerCase() === 'content-type');
  if (typeof raw === 'string') return { body: raw };
  if (raw instanceof URLSearchParams) {
    if (!hasContentType()) {
      headers['Content-Type'] = 'application/x-www-form-urlencoded;charset=UTF-8';
    }
    return { body: raw.toString() };
  }
  if (raw instanceof Blob) {
    const bytes = new Uint8Array(await raw.arrayBuffer());
    return { descriptor: { kind: 'blob', data: bytesToBase64(bytes), type: raw.type || '' } };
  }
  if (raw instanceof ArrayBuffer) {
    return { descriptor: { kind: 'bytes', data: bytesToBase64(new Uint8Array(raw)) } };
  }
  if (ArrayBuffer.isView(raw)) {
    const bytes = new Uint8Array(raw.buffer, raw.byteOffset, raw.byteLength);
    return { descriptor: { kind: 'bytes', data: bytesToBase64(bytes) } };
  }
  if (typeof FormData !== 'undefined' && raw instanceof FormData) {
    return { descriptor: await serializeFormData(raw) };
  }
  if (!hasContentType()) headers['Content-Type'] = 'application/json';
  return { body: JSON.stringify(raw) };
}

function bodyReconstruction(descriptor) {
  if (!descriptor) return '';
  return (
    'const __b64 = (s) => { const bin = atob(s); const n = bin.length; ' +
    'const u = new Uint8Array(n); for (let i = 0; i < n; i++) u[i] = bin.charCodeAt(i); return u; };' +
    `const __body = ${JSON.stringify(descriptor)};` +
    "if (__body.kind === 'bytes') { __init.body = __b64(__body.data); }" +
    "else if (__body.kind === 'blob') { __init.body = new Blob([__b64(__body.data)], { type: __body.type }); }" +
    "else if (__body.kind === 'formdata') { const __fd = new FormData(); " +
    'for (const e of __body.entries) { ' +
    'if (e.file) { __fd.append(e.name, new Blob([__b64(e.file.data)], { type: e.file.type }), e.file.filename); } ' +
    'else { __fd.append(e.name, e.value); } } __init.body = __fd; }'
  );
}

function responseHandling(responseType) {
  return (
    'const h = {};' +
    'r.headers.forEach((v, k) => { h[k] = v; });' +
    "const ct = r.headers.get('content-type') || '';" +
    `const __rt = ${JSON.stringify(responseType ?? null)};` +
    'const __ctl = ct.toLowerCase();' +
    "const __binPrefixes = ['image/','audio/','video/','application/octet-stream'," +
    "'application/pdf','application/protobuf','application/x-protobuf','application/wasm','application/zip'];" +
    "const __isXml = __ctl.indexOf('+xml') !== -1 || __ctl.indexOf('application/xml') === 0 || __ctl.indexOf('text/xml') === 0;" +
    "const __isBinary = __rt === 'binary' || (__rt !== 'text' && __rt !== 'json' && !__isXml && " +
    '__binPrefixes.some((p) => __ctl.indexOf(p) === 0));' +
    'const __meta = { ok: r.ok, status: r.status, statusText: r.statusText, ' +
    'url: r.url, redirected: !!r.redirected, headers: h };' +
    'if (__isBinary) {' +
    'const __u = new Uint8Array(await r.arrayBuffer());' +
    "let __s = ''; const __cs = 0x8000;" +
    'for (let __i = 0; __i < __u.length; __i += __cs) { ' +
    '__s += String.fromCharCode.apply(null, __u.subarray(__i, __i + __cs)); }' +
    "return Object.assign(__meta, { body: btoa(__s), bodyEncoding: 'base64' });" +
    '}' +
    'const t = await r.text();' +
    'let b;' +
    "const __jsonWanted = __rt === 'json' || (__rt !== 'text' && ct.indexOf('application/json') !== -1);" +
    'if (__jsonWanted) { if (!t) { b = null; } else { try { b = JSON.parse(t); } catch (e) { b = t; } } }' +
    'else { b = t; }' +
    'return Object.assign(__meta, { body: b });'
  );
}

export async function buildBrowserFetchScript(url, opts = {}) {
  const headers = {};
  for (const [key, value] of Object.entries(opts.headers ?? {})) {
    if (typeof value === 'string') headers[key] = value;
  }
  const method = typeof opts.method === 'string' ? opts.method : 'GET';
  const credentials =
    opts.credentials === 'same-origin' || opts.credentials === 'omit'
      ? opts.credentials
      : 'include';
  const raw = opts.body;
  const { body, descriptor } =
    raw === undefined || raw === null ? {} : await serializeBody(raw, headers);
  const init = { method, credentials, headers };
  if (body !== undefined) init.body = body;
  if (opts.mode !== undefined) init.mode = opts.mode;
  if (opts.cache !== undefined) init.cache = opts.cache;
  if (opts.redirect !== undefined) init.redirect = opts.redirect;
  if (opts.referrer !== undefined) init.referrer = opts.referrer;
  if (opts.referrerPolicy !== undefined) init.referrerPolicy = opts.referrerPolicy;
  if (opts.integrity !== undefined) init.integrity = opts.integrity;
  if (opts.keepalive !== undefined) init.keepalive = opts.keepalive;
  const timeoutMs =
    typeof opts.timeoutMs === 'number' && Number.isFinite(opts.timeoutMs) && opts.timeoutMs > 0
      ? Math.round(opts.timeoutMs)
      : 0;
  const timeoutScript = timeoutMs > 0 ? `__init.signal = AbortSignal.timeout(${timeoutMs});` : '';
  return (
    '(async () => {' +
    `const __init = ${JSON.stringify(init)};` +
    timeoutScript +
    bodyReconstruction(descriptor) +
    `const r = await fetch(${JSON.stringify(url)}, __init);` +
    responseHandling(opts.responseType) +
    '})()'
  );
}
