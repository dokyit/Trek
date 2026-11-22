// The web remote's plain code, in Node: the WebSocket framing it speaks on its WebTransport stream,
// the pairing link and the addresses in it, and the headers the site serves it with.
//
//   node script/site/test/run.mjs

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { FrameReader, OP, closeFrame, encodeFrame, textFrame } from '../../../site/remote/js/frame.js';
import { liveHashes, mayReplace, parsePairing, parseRoute, sessionUrl, toBase64 } from '../../../site/remote/js/link.js';
import { ROOT, headersFor, parseHeaders } from '../serve.mjs';

const enc = new TextEncoder();

/** A frame as the server writes one: unmasked, FIN as asked. */
function serverFrame(op, payload, fin = true) {
  const len = payload.length;
  const head = len < 126 ? 2 : len < 65536 ? 4 : 10;
  const out = new Uint8Array(head + len);
  out[0] = (fin ? 0x80 : 0) | op;
  if (len < 126) out[1] = len;
  else if (len < 65536) {
    out[1] = 126;
    out[2] = len >> 8;
    out[3] = len & 255;
  } else {
    out[1] = 127;
    new DataView(out.buffer).setUint32(6, len);
  }
  out.set(payload, head);
  return out;
}

/** What a server reads from a client's frame: its opcode and unmasked payload. */
function unmask(frame) {
  assert.ok(frame[1] & 0x80, 'a client masks every frame');
  let len = frame[1] & 0x7f;
  let at = 2;
  if (len === 126) {
    len = (frame[2] << 8) | frame[3];
    at = 4;
  } else if (len === 127) {
    len = new DataView(frame.buffer, frame.byteOffset).getUint32(6);
    at = 10;
  }
  const mask = frame.subarray(at, at + 4);
  const payload = frame.slice(at + 4, at + 4 + len);
  for (let i = 0; i < payload.length; i++) payload[i] ^= mask[i & 3];
  return { fin: !!(frame[0] & 0x80), op: frame[0] & 0x0f, payload };
}

test('a client frame is masked and reads back as it was, at every length form', () => {
  for (const size of [0, 5, 125, 126, 1000, 65535, 65536, 200_000]) {
    const payload = new Uint8Array(size).map((_, i) => (i * 7) & 255);
    const read = unmask(encodeFrame(OP.BINARY, payload));
    assert.equal(read.op, OP.BINARY);
    assert.ok(read.fin);
    assert.deepEqual(read.payload, payload, `${size} bytes`);
  }
  const text = unmask(textFrame('{"type":"hello","name":"Tobias’s phone"}'));
  assert.equal(new TextDecoder().decode(text.payload), '{"type":"hello","name":"Tobias’s phone"}');
  const close = unmask(closeFrame(1000, 'bye'));
  assert.equal(close.op, OP.CLOSE);
  assert.equal((close.payload[0] << 8) | close.payload[1], 1000);
});

test('the server’s frames read whole however the stream splits them', () => {
  const message = JSON.stringify({ type: 'snapshot', threads: Array.from({ length: 300 }, (_, i) => ({ id: `t${i}`, title: `Thread ${i} — “quoted”` })) });
  const bytes = enc.encode(message);
  const third = Math.floor(bytes.length / 3);
  // A ping between the pieces of a message in three frames, then a short one.
  const stream = [
    serverFrame(OP.TEXT, bytes.subarray(0, third), false),
    serverFrame(OP.PING, enc.encode('hi')),
    serverFrame(OP.CONTINUATION, bytes.subarray(third, 2 * third), false),
    serverFrame(OP.CONTINUATION, bytes.subarray(2 * third), true),
    serverFrame(OP.TEXT, enc.encode('{"type":"pong"}')),
  ];
  const all = new Uint8Array(stream.reduce((n, f) => n + f.length, 0));
  let at = 0;
  for (const f of stream) {
    all.set(f, at);
    at += f.length;
  }
  for (const step of [1, 2, 3, 7, 100, 4096, all.length]) {
    const reader = new FrameReader();
    const got = [];
    for (let i = 0; i < all.length; i += step) got.push(...reader.push(all.subarray(i, i + step)));
    assert.equal(got.length, 3, `in pieces of ${step}`);
    assert.equal(got[0].op, OP.PING);
    assert.equal(new TextDecoder().decode(got[0].data), 'hi');
    assert.equal(got[1].op, OP.TEXT);
    assert.equal(got[1].text, message, `the whole message, in pieces of ${step}`);
    assert.equal(got[2].text, '{"type":"pong"}');
  }
});

test('a close says its code and reason; nonsense is refused', () => {
  const reader = new FrameReader();
  const payload = new Uint8Array([0x03, 0xe8, ...enc.encode('going away')]);
  assert.deepEqual(reader.push(serverFrame(OP.CLOSE, payload)), [{ op: OP.CLOSE, code: 1000, reason: 'going away' }]);
  assert.throws(() => new FrameReader().push(serverFrame(OP.CONTINUATION, enc.encode('x'))), /continuation/);
  assert.throws(() => new FrameReader().push(new Uint8Array([0xc1, 0x00])), /reserved/);
  assert.throws(() => new FrameReader(10).push(serverFrame(OP.TEXT, new Uint8Array(11))), /11 bytes/);
});

// ---------------------------------------------------------------------------------------------
// The pairing link

function link(payload) {
  const b64 = toBase64(enc.encode(JSON.stringify(payload))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  return `#p=${b64}`;
}

const HASH_A = toBase64(new Uint8Array(32).fill(1));
const HASH_B = toBase64(new Uint8Array(32).fill(2));

test('a pairing link reads as Trek writes it (trek_remote::web::pairing_link)', () => {
  const read = parsePairing(link({ v: 1, code: 'K7Q2-9XMV', name: 'Toby’s PC', hid: '7f3c', routes: ['192.168.1.20:7420', '[fd7a:115c::1]:7420'], certs: [HASH_A, HASH_B] }));
  assert.deepEqual(read, { code: 'K7Q2-9XMV', name: 'Toby’s PC', hostId: '7f3c', routes: ['192.168.1.20:7420', '[fd7a:115c::1]:7420'], certs: [{ hash: HASH_A }, { hash: HASH_B }], push: false });
});

test('anything else isn’t a pairing link', () => {
  assert.equal(parsePairing(''), null);
  assert.equal(parsePairing('#t=abc'), null);
  assert.equal(parsePairing('#p=not-base64!'), null);
  assert.equal(parsePairing(link({ v: 2, code: 'X', routes: ['1.2.3.4:1'], certs: [HASH_A] })), null, 'another version');
  assert.equal(parsePairing(link({ v: 1, code: 'X', routes: [], certs: [HASH_A] })), null, 'no address');
  assert.equal(parsePairing(link({ v: 1, code: 'X', routes: ['1.2.3.4:1'], certs: ['short'] })), null, 'no whole hash');
  assert.equal(parsePairing(link({ v: 1, routes: ['1.2.3.4:1'], certs: [HASH_A] })), null, 'no code');
});

test('addresses become WebTransport URLs, IPv6 in brackets', () => {
  assert.deepEqual(parseRoute('192.168.1.20:7420'), { host: '192.168.1.20', port: 7420 });
  assert.deepEqual(parseRoute('[fd7a:115c::1]:7420'), { host: '[fd7a:115c::1]', port: 7420 });
  assert.deepEqual(parseRoute('my-pc.tail1234.ts.net:7420'), { host: 'my-pc.tail1234.ts.net', port: 7420 });
  assert.equal(sessionUrl('[::1]:7420'), 'https://[::1]:7420/trek');
  assert.equal(sessionUrl('10.0.0.2:7420'), 'https://10.0.0.2:7420/trek');
  for (const bad of ['', 'nohost', ':7420', '1.2.3.4:', '1.2.3.4:99999', '[::1]7420', 'a b:1', 'x/y:1']) assert.equal(parseRoute(bad), null, bad);
});

test('a pairing only takes a kept computer’s place when it pinned that computer’s certificates', () => {
  const HASH_C = toBase64(new Uint8Array(32).fill(3));
  const kept = { hostId: '7f3c', name: 'Toby’s PC', token: 't', certs: [{ hash: HASH_A, from: 1, to: 2 }, { hash: HASH_B, from: 2, to: 3 }] };
  assert.ok(mayReplace(null, [{ hash: HASH_C }]), 'nothing kept under that id');
  assert.ok(mayReplace(kept, [{ hash: HASH_A }, { hash: HASH_B }]), 'the same computer, paired again');
  // A link anyone can write, naming the kept computer's id: its certificates are others'.
  assert.ok(!mayReplace(kept, [{ hash: HASH_C }]));
  assert.ok(!mayReplace(kept, [{ hash: HASH_A }, { hash: HASH_C }]), 'one of its own beside another isn’t proof');
  assert.ok(!mayReplace(kept, []));
  assert.ok(!mayReplace({ ...kept, certs: undefined }, [{ hash: HASH_A }]));
});

test('certificates that have ended aren’t pinned', () => {
  const day = 86_400_000;
  const now = 1_800_000_000_000;
  const certs = [{ hash: 'old', from: now - 20 * day, to: now - 2 * day }, { hash: 'now', from: now - day, to: now + 12 * day }, { hash: 'link' }];
  assert.deepEqual(liveHashes(certs, now).map((c) => c.hash), ['now', 'link']);
});

// ---------------------------------------------------------------------------------------------
// The site's headers

test('the web remote gets its own security policy; the rest of the site keeps the strict one', () => {
  const rules = parseHeaders(readFileSync(join(ROOT, '_headers'), 'utf8'));
  const remote = headersFor(rules, '/remote/');
  const csp = remote['Content-Security-Policy'];
  assert.ok(csp, 'a policy');
  assert.ok(!csp.includes(','), `one policy, not two joined: ${csp}`);
  assert.match(csp, /connect-src 'self' https:/);
  assert.match(csp, /worker-src 'self'/);
  assert.match(csp, /script-src 'self';/, 'scripts are the site’s own');
  assert.match(remote['Permissions-Policy'], /microphone=\(self\)/);
  assert.equal(remote['Cache-Control'], 'no-cache');
  assert.equal(headersFor(rules, '/remote/js/client.js')['Content-Security-Policy'], csp);

  const home = headersFor(rules, '/');
  assert.match(home['Content-Security-Policy'], /connect-src https:\/\/api\.github\.com/);
  assert.doesNotMatch(home['Content-Security-Policy'], /worker-src/);
  assert.match(home['Permissions-Policy'], /microphone=\(\)/);
});

// ---------------------------------------------------------------------------------------------
// The service worker's list

test('the service worker keeps every file the remote loads, and nothing that isn’t there', async () => {
  const { listed, shell } = await import('../remote-shell.mjs');
  assert.deepEqual(listed(), shell(), 'out of date: node script/site/remote-shell.mjs');
});
