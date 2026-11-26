// The Browser's passkey scripts, run together outside a web view (Node 19 or later):
//
//     node crates/trek-app/src/panels/browser/fixtures/passkeys_page_test.js
//
// `passkeys_page.js` (the page's side) and `passkeys_bridge.js` (the bridge) share one stand-in
// document, with a stand-in for Trek's native handler: what the page's calls send, what its
// objects are made of, aborts, errors, and what the bridge refuses to pass on.
const fs = require('fs');
const path = require('path');
const pagePath = path.join(__dirname, '..', 'passkeys_page.js');
const bridgePath = path.join(__dirname, '..', 'passkeys_bridge.js');
// One "window" shared by two scripts, as the page world and the bridge's world share a DOM.
const document = new EventTarget();
// The bridge watches the document's root to listen again after `document.open()`; here it is
// told by hand (`reopened`), as a page reopening its document would have it told.
let reopened = () => {};
globalThis.MutationObserver = class { constructor(f) { reopened = f; } observe() {} };
class CredentialsContainer { create() { return Promise.reject(new DOMException('native', 'NotAllowedError')); } get() { return Promise.reject(new DOMException('native', 'NotAllowedError')); } }
class PublicKeyCredential {}
class AuthenticatorResponse {}
class AuthenticatorAttestationResponse extends AuthenticatorResponse {}
class AuthenticatorAssertionResponse extends AuthenticatorResponse {}
const navigator = { credentials: new CredentialsContainer() };
const seen = [];
let replyWith = null;
const window = {
  isSecureContext: true, document, navigator, PublicKeyCredential, AuthenticatorResponse,
  AuthenticatorAttestationResponse, AuthenticatorAssertionResponse,
  btoa, atob, crypto,
  webkit: { messageHandlers: { trekPasskeys: { postMessage: (text) => { seen.push(JSON.parse(text)); return replyWith(JSON.parse(text)); } } } },
};
window.top = window; window.window = window;
const run = (path, extra) => new Function('window', 'document', 'navigator', 'crypto', ...Object.keys(extra), fs.readFileSync(path, 'utf8').replace('__TREK_CONDITIONAL__', 'true'))(window, document, navigator, crypto, ...Object.values(extra));
run(bridgePath, {});
run(pagePath, {});
const b64 = (s) => Buffer.from(s).toString('base64url');
(async () => {
  const assert = require('assert');
  assert.strictEqual(await PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable(), true);
  assert.strictEqual(await PublicKeyCredential.isConditionalMediationAvailable(), true);
  assert.strictEqual((await PublicKeyCredential.getClientCapabilities()).hybridTransport, false);
  // create
  replyWith = async (m) => JSON.stringify({ ok: { id: b64('cred'), rawId: b64('cred'), type: 'public-key', authenticatorAttachment: 'platform',
    response: { clientDataJSON: b64('{}'), attestationObject: b64('att'), authenticatorData: b64('data'), transports: ['internal'], publicKey: b64('spki'), publicKeyAlgorithm: -7 },
    clientExtensionResults: { credProps: { rk: true } } } });
  const cred = await navigator.credentials.create({ publicKey: {
    rp: { name: 'Example' }, user: { id: new Uint8Array([1, 2, 3]), name: 'ada', displayName: 'Ada' },
    challenge: new Uint8Array([255, 254, 253]).buffer, pubKeyCredParams: [{ type: 'public-key', alg: -7 }],
    excludeCredentials: [{ type: 'public-key', id: new DataView(new Uint8Array([9, 9]).buffer) }],
    authenticatorSelection: { residentKey: 'required' }, extensions: { credProps: true } } });
  const sent = seen.pop();
  assert.strictEqual(sent.kind, 'create');
  assert.strictEqual(sent.publicKey.challenge, '__79');
  assert.strictEqual(sent.publicKey.user.id, 'AQID');
  assert.strictEqual(sent.publicKey.excludeCredentials[0].id, 'CQk');
  assert.strictEqual(sent.publicKey.rp.id, undefined);
  assert.deepStrictEqual(sent.publicKey.extensions, { credProps: true });
  assert.ok(cred instanceof PublicKeyCredential && cred.response instanceof AuthenticatorAttestationResponse);
  assert.strictEqual(Buffer.from(cred.rawId).toString(), 'cred');
  assert.strictEqual(Buffer.from(cred.response.getPublicKey()).toString(), 'spki');
  assert.deepStrictEqual(cred.response.getTransports(), ['internal']);
  assert.strictEqual(cred.getClientExtensionResults().credProps.rk, true);
  assert.strictEqual(JSON.parse(JSON.stringify(cred)).response.attestationObject, b64('att'));
  assert.strictEqual(cred.response.clientDataJSON, cred.response.clientDataJSON, 'same object each time');
  // get, with an error
  replyWith = async () => JSON.stringify({ error: { name: 'SecurityError', message: 'no' } });
  await assert.rejects(navigator.credentials.get({ publicKey: { challenge: new Uint8Array(4), rpId: 'evil.example' } }), (e) => e instanceof DOMException && e.name === 'SecurityError');
  replyWith = async () => JSON.stringify({ error: { name: 'TypeError', message: 'bad' } });
  await assert.rejects(navigator.credentials.get({ publicKey: { challenge: new Uint8Array(4) } }), TypeError);
  // a challenge that isn't a buffer never leaves the page
  const before = seen.length;
  await assert.rejects(navigator.credentials.get({ publicKey: { challenge: 'text' } }), TypeError);
  assert.strictEqual(seen.length, before);
  // anything but publicKey is WebKit's
  await assert.rejects(navigator.credentials.get({ password: true }), (e) => e.message === 'native');
  // abort: the page's promise rejects at once and Trek is told
  let release;
  replyWith = (m) => m.kind === 'cancel' ? Promise.resolve('{"ok":true}') : new Promise((r) => { release = r; });
  const controller = new AbortController();
  const pending = navigator.credentials.get({ publicKey: { challenge: new Uint8Array(4) }, mediation: 'conditional', signal: controller.signal });
  await new Promise((r) => setTimeout(r, 10));
  const asked = seen[seen.length - 1];
  assert.strictEqual(asked.mediation, 'conditional');
  controller.abort();
  await assert.rejects(pending, (e) => e.name === 'AbortError');
  await new Promise((r) => setTimeout(r, 10));
  const cancel = seen[seen.length - 1];
  assert.deepStrictEqual([cancel.kind, cancel.of], ['cancel', asked.id]);
  // an already aborted signal asks nothing
  const n = seen.length;
  await assert.rejects(navigator.credentials.get({ publicKey: { challenge: new Uint8Array(4) }, signal: AbortSignal.abort() }), (e) => e.name === 'AbortError');
  assert.strictEqual(seen.length, n);
  // the bridge passes on nothing but its three kinds, and nothing oversized
  document.dispatchEvent(new CustomEvent('trek-passkeys:ask', { detail: JSON.stringify({ id: 'x', kind: 'store' }) }));
  document.dispatchEvent(new CustomEvent('trek-passkeys:ask', { detail: { id: 'x', kind: 'get' } }));
  document.dispatchEvent(new CustomEvent('trek-passkeys:ask', { detail: JSON.stringify({ id: 'x'.repeat(100), kind: 'get' }) }));
  document.dispatchEvent(new CustomEvent('trek-passkeys:ask', { detail: 'x'.repeat(300000) }));
  assert.strictEqual(seen.length, n);
  // a document opened again loses its listeners (a fresh target stands in for that here), and
  // both sides listen again: the bridge when the root changes, the page's side as it asks
  const listeners = [];
  const add = document.addEventListener.bind(document);
  document.addEventListener = (type, f) => { listeners.push([type, f]); add(type, f); };
  reopened();
  assert.deepStrictEqual(listeners.map((l) => l[0]), ['trek-passkeys:ask']);
  replyWith = async () => JSON.stringify({ error: { name: 'NotAllowedError', message: 'no' } });
  await assert.rejects(navigator.credentials.get({ publicKey: { challenge: new Uint8Array(4) } }), (e) => e.name === 'NotAllowedError');
  // what reaches Trek is well-formed text, whatever the page put in its event
  document.dispatchEvent(new CustomEvent('trek-passkeys:ask', { detail: '{"id":"s","kind":"get","x":"\ud800"}' }));
  await new Promise((r) => setTimeout(r, 10));
  assert.strictEqual(seen[seen.length - 1].x, '\ud800');
  // a forged answer for an unknown id changes nothing
  document.dispatchEvent(new CustomEvent('trek-passkeys:answer', { detail: JSON.stringify({ id: 'nope', ok: {} }) }));
  console.log('page and bridge: all checks passed');
})().catch((e) => { console.error(e); process.exit(1); });
