// The pairing link Trek's Settings › Phone shows for a phone's browser
// (`https://trekdesktop.com/remote/#p=<base64url JSON>`, `trek_remote::web::pairing_link`), and the
// addresses in it. Everything after `#` stays on the phone: a browser never sends it to the site.
//
// Plain code: `test/link.test.mjs` runs it in Node.

/** base64 or base64url, padded or not, to bytes. */
export function fromBase64(text) {
  const plain = text.replace(/-/g, '+').replace(/_/g, '/');
  const padded = plain + '='.repeat((4 - (plain.length % 4)) % 4);
  const binary = atob(padded);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}

export function toBase64(bytes) {
  let binary = '';
  for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
  return btoa(binary);
}

/**
 * What a pairing link's fragment (`#p=…`, with or without the `#`) holds, checked:
 * `{ code, name, hostId, routes, certs }`, or `null` when it isn't a pairing link.
 */
export function parsePairing(fragment) {
  const raw = (fragment || '').replace(/^#/, '');
  const params = new URLSearchParams(raw);
  const encoded = params.get('p');
  if (!encoded) return null;
  let json;
  try {
    json = JSON.parse(new TextDecoder().decode(fromBase64(encoded)));
  } catch {
    return null;
  }
  if (!json || typeof json !== 'object' || json.v !== 1) return null;
  const code = typeof json.code === 'string' ? json.code.trim() : '';
  const routes = Array.isArray(json.routes) ? json.routes.filter((r) => typeof r === 'string' && parseRoute(r)) : [];
  const certs = Array.isArray(json.certs)
    ? json.certs
        .map((c) => (typeof c === 'string' ? { hash: c } : c && typeof c.hash === 'string' ? c : null))
        .filter((c) => c && isHash(c.hash))
    : [];
  if (!code || routes.length === 0 || certs.length === 0) return null;
  return {
    code,
    name: typeof json.name === 'string' && json.name ? json.name : 'Your computer',
    hostId: typeof json.hid === 'string' ? json.hid : '',
    routes,
    certs,
  };
}

/** A SHA-256 hash in base64: 32 bytes. */
export function isHash(text) {
  try {
    return fromBase64(text).length === 32;
  } catch {
    return false;
  }
}

/**
 * `host:port` (`192.168.1.20:7420`, `[fd7a:115c::1]:7420`, `my-pc.tail1234.ts.net:7420`) to
 * `{ host, port }`, the host as a URL takes it (an IPv6 address in brackets); `null` if it isn't one.
 */
export function parseRoute(route) {
  const text = String(route).trim();
  let host;
  let port;
  if (text.startsWith('[')) {
    const close = text.indexOf(']');
    if (close < 0 || text[close + 1] !== ':') return null;
    host = text.slice(0, close + 1);
    port = text.slice(close + 2);
  } else {
    const colon = text.lastIndexOf(':');
    if (colon <= 0) return null;
    host = text.slice(0, colon);
    port = text.slice(colon + 1);
    if (host.includes(':')) host = `[${host}]`;
  }
  const number = Number(port);
  if (!/^\d+$/.test(port) || number < 1 || number > 65535) return null;
  if (!/^[\w.\-:[\]%]+$/.test(host)) return null;
  return { host, port: number };
}

/** The address a page opens a WebTransport session at, for one route. */
export function sessionUrl(route, path = '/trek') {
  const parsed = parseRoute(route);
  return parsed ? `https://${parsed.host}:${parsed.port}${path}` : null;
}

/**
 * The certificate hashes worth pinning at `now`: every one still valid half a day from now (a
 * phone's clock may be off), newest last. Hashes from a pairing link have no dates; they're kept
 * until the series replaces them.
 */
export function liveHashes(certs, now = Date.now()) {
  const halfDay = 12 * 60 * 60 * 1000;
  return certs.filter((c) => !c.to || c.to > now - halfDay);
}
