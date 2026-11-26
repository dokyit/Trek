// Trek's passkeys, the page's side: installed in the page's own world before any of its scripts
// run (main frame only). It replaces what WebKit refuses an app's web view (`navigator
// .credentials.create/get` with `publicKey`, and the `PublicKeyCredential` feature checks) with
// calls to Trek's authenticator, over events only the bridge script (`passkeys_bridge.js`, in a
// world of its own) listens to.
//
// Nothing here is trusted by Trek: a page can call or rewrite all of it, exactly as it can call
// `navigator.credentials`. The request is checked natively against the origin WebKit reports for
// the frame, and only the user (Trek's bar, then Touch ID) lets it through.
(() => {
  'use strict';
  if (!window.isSecureContext || window.top !== window) return;
  const CONDITIONAL = __TREK_CONDITIONAL__;
  const ASK = 'trek-passkeys:ask';
  const ANSWER = 'trek-passkeys:answer';

  // What the page could replace later, kept as it was at document start.
  const doc = document;
  const listen = doc.addEventListener.bind(doc);
  const dispatch = doc.dispatchEvent.bind(doc);
  const Event_ = CustomEvent;
  const { stringify, parse } = JSON;
  const { fromCharCode } = String;
  const btoa_ = window.btoa.bind(window);
  const atob_ = window.atob.bind(window);
  const Promise_ = Promise;
  const TypeError_ = TypeError;
  const DOMException_ = DOMException;
  const Uint8Array_ = Uint8Array;
  const define = Object.defineProperty;
  const tag = Object.prototype.toString;

  const pending = new Map();
  const prefix = Array.from(crypto.getRandomValues(new Uint32Array(3)), (n) => n.toString(36)).join('');
  let count = 0;
  const onAnswer = (event) => {
    let message;
    try { message = parse(event.detail); } catch (_) { return; }
    const settle = message && pending.get(message.id);
    if (!settle) return;
    pending.delete(message.id);
    settle(message);
  };
  const post = (message) => {
    // Listening again each time: `document.open()` takes a document's listeners away, and the
    // same listener is never added twice.
    listen(ANSWER, onAnswer);
    dispatch(new Event_(ASK, { detail: stringify(message) }));
  };
  const ask = (message) => new Promise_((resolve) => {
    pending.set(message.id, resolve);
    post(message);
  });

  // base64url, as WebAuthn's JSON forms carry binary values.
  const bytesOf = (value, what) => {
    const kind = tag.call(value);
    if (kind === '[object ArrayBuffer]') return new Uint8Array_(value);
    if (ArrayBuffer.isView(value)) return new Uint8Array_(value.buffer, value.byteOffset, value.byteLength);
    throw new TypeError_(`${what} must be an ArrayBuffer or a view of one.`);
  };
  const encode = (value, what) => {
    const bytes = bytesOf(value, what);
    let text = '';
    for (let i = 0; i < bytes.length; i++) text += fromCharCode(bytes[i]);
    return btoa_(text).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  };
  const decode = (text) => {
    const raw = atob_(String(text).replace(/-/g, '+').replace(/_/g, '/'));
    const bytes = new Uint8Array_(raw.length);
    for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
    return bytes.buffer;
  };
  const text = (value) => (value === undefined || value === null ? undefined : String(value));
  const binary = (value, what) => (value === undefined || value === null ? undefined : encode(value, what));
  const list = (value, what) => {
    if (value === undefined || value === null) return undefined;
    if (typeof value[Symbol.iterator] !== 'function') throw new TypeError_(`${what} must be a list.`);
    return Array.from(value);
  };
  const descriptors = (value, what) => {
    const items = list(value, what);
    return items && items.map((d) => ({
      type: text(d && d.type),
      id: binary(d && d.id, `${what}.id`),
    }));
  };
  const extensions = (value) => {
    if (!value || typeof value !== 'object') return undefined;
    const out = {};
    if (value.credProps === true) out.credProps = true;
    if (value.appid !== undefined && value.appid !== null) out.appid = String(value.appid);
    return out;
  };

  // The option dictionaries, as their JSON forms (binary members as base64url).
  const creation = (options) => {
    if (!options || typeof options !== 'object') throw new TypeError_('publicKey must be a dictionary.');
    const rp = options.rp || {};
    const user = options.user || {};
    const selection = options.authenticatorSelection;
    const params = list(options.pubKeyCredParams, 'pubKeyCredParams');
    return {
      rp: options.rp === undefined ? undefined : { id: text(rp.id), name: text(rp.name) },
      user: options.user === undefined ? undefined : {
        id: binary(user.id, 'user.id'),
        name: text(user.name),
        displayName: text(user.displayName),
      },
      challenge: binary(options.challenge, 'challenge'),
      pubKeyCredParams: params && params.map((p) => ({ type: text(p && p.type), alg: Number(p && p.alg) })),
      excludeCredentials: descriptors(options.excludeCredentials, 'excludeCredentials'),
      authenticatorSelection: selection && typeof selection === 'object' ? {
        authenticatorAttachment: text(selection.authenticatorAttachment),
        residentKey: text(selection.residentKey),
        requireResidentKey: selection.requireResidentKey === true,
        userVerification: text(selection.userVerification),
      } : undefined,
      attestation: text(options.attestation),
      extensions: extensions(options.extensions),
    };
  };
  const request = (options) => {
    if (!options || typeof options !== 'object') throw new TypeError_('publicKey must be a dictionary.');
    return {
      challenge: binary(options.challenge, 'challenge'),
      rpId: text(options.rpId),
      allowCredentials: descriptors(options.allowCredentials, 'allowCredentials'),
      userVerification: text(options.userVerification),
      extensions: extensions(options.extensions),
    };
  };

  // The page's objects, made of Trek's answer: real `PublicKeyCredential`s to `instanceof`,
  // their members as own values over WebKit's (whose getters only know WebKit's own).
  const Credential_ = window.PublicKeyCredential || class PublicKeyCredential {};
  const Response_ = window.AuthenticatorResponse || class AuthenticatorResponse {};
  const Attestation_ = window.AuthenticatorAttestationResponse
    || class AuthenticatorAttestationResponse extends Response_ {};
  const Assertion_ = window.AuthenticatorAssertionResponse
    || class AuthenticatorAssertionResponse extends Response_ {};
  for (const [name, value] of [
    ['PublicKeyCredential', Credential_],
    ['AuthenticatorResponse', Response_],
    ['AuthenticatorAttestationResponse', Attestation_],
    ['AuthenticatorAssertionResponse', Assertion_],
  ]) {
    if (!window[name]) define(window, name, { value, writable: true, configurable: true });
  }
  const make = (proto, members) => {
    const object = Object.create(proto);
    for (const key of Object.keys(members)) {
      define(object, key, { value: members[key], enumerable: true, configurable: true });
    }
    return object;
  };
  const credential = (answer, response) => make(Credential_.prototype, {
    id: answer.id,
    rawId: decode(answer.rawId),
    type: 'public-key',
    authenticatorAttachment: answer.authenticatorAttachment,
    response,
    getClientExtensionResults: () => parse(stringify(answer.clientExtensionResults || {})),
    toJSON: () => parse(stringify(answer)),
  });
  const registration = (answer) => {
    const r = answer.response;
    return credential(answer, make(Attestation_.prototype, {
      clientDataJSON: decode(r.clientDataJSON),
      attestationObject: decode(r.attestationObject),
      getTransports: () => Array.from(r.transports),
      getAuthenticatorData: () => decode(r.authenticatorData),
      getPublicKey: () => decode(r.publicKey),
      getPublicKeyAlgorithm: () => r.publicKeyAlgorithm,
    }));
  };
  const assertion = (answer) => {
    const r = answer.response;
    return credential(answer, make(Assertion_.prototype, {
      clientDataJSON: decode(r.clientDataJSON),
      authenticatorData: decode(r.authenticatorData),
      signature: decode(r.signature),
      userHandle: r.userHandle ? decode(r.userHandle) : null,
    }));
  };
  const failure = (error) => {
    const name = error && error.name;
    const message = (error && error.message) || 'The request was cancelled.';
    return name === 'TypeError' ? new TypeError_(message) : new DOMException_(message, name || 'NotAllowedError');
  };

  const run = (kind, options) => new Promise_((resolve, reject) => {
    const signal = options.signal;
    const aborted = () => (signal.reason !== undefined
      ? signal.reason
      : new DOMException_('The operation was aborted.', 'AbortError'));
    if (signal && signal.aborted) { reject(aborted()); return; }
    let publicKey;
    try {
      publicKey = kind === 'create' ? creation(options.publicKey) : request(options.publicKey);
    } catch (error) { reject(error); return; }
    const id = `${prefix}-${++count}`;
    let done = false;
    const onAbort = () => {
      if (done) return;
      done = true;
      pending.delete(id);
      post({ id: `${id}-x`, kind: 'cancel', of: id });
      reject(aborted());
    };
    if (signal) signal.addEventListener('abort', onAbort, { once: true });
    ask({ id, kind, mediation: text(options.mediation), publicKey }).then((message) => {
      if (done) return;
      done = true;
      if (signal) signal.removeEventListener('abort', onAbort);
      try {
        if (message.error || !message.ok) reject(failure(message.error));
        else resolve(kind === 'create' ? registration(message.ok) : assertion(message.ok));
      } catch (error) { reject(failure()); }
    });
  });

  // navigator.credentials: requests for a public key are Trek's, everything else WebKit's.
  const container = navigator.credentials;
  const home = container ? Object.getPrototypeOf(container) : null;
  const original = { create: home && home.create, get: home && home.get };
  const replace = (kind) => function (options) {
    if (options && typeof options === 'object' && options.publicKey !== undefined) return run(kind, options);
    if (original[kind]) return original[kind].apply(this, arguments);
    return Promise_.reject(new DOMException_('Only passkeys are kept here.', 'NotSupportedError'));
  };
  const methods = { create: replace('create'), get: replace('get') };
  if (home) {
    for (const kind of ['create', 'get']) {
      define(home, kind, { value: methods[kind], writable: true, enumerable: true, configurable: true });
    }
  } else {
    define(navigator, 'credentials', {
      value: {
        create: methods.create,
        get: methods.get,
        preventSilentAccess: () => Promise_.resolve(),
        store: () => Promise_.reject(new DOMException_('Only passkeys are kept here.', 'NotSupportedError')),
      },
      enumerable: true,
      configurable: true,
    });
  }

  // The feature checks sites decide by. There is an authenticator that verifies the user
  // (Touch ID or the password) on this computer, and nothing else: no phone, no security key.
  const fixed = (name, value) => define(Credential_, name, {
    value: () => Promise_.resolve(typeof value === 'function' ? value() : value),
    writable: true,
    configurable: true,
  });
  fixed('isUserVerifyingPlatformAuthenticatorAvailable', true);
  fixed('isConditionalMediationAvailable', CONDITIONAL);
  fixed('getClientCapabilities', () => ({
    conditionalCreate: false,
    conditionalGet: CONDITIONAL,
    'extension:appid': true,
    'extension:credProps': true,
    hybridTransport: false,
    passkeyPlatformAuthenticator: true,
    relatedOrigins: false,
    signalAllAcceptedCredentials: false,
    signalCurrentUserDetails: false,
    signalUnknownCredential: false,
    userVerifyingPlatformAuthenticator: true,
  }));
  // The JSON forms' readers, where WebKit has none yet.
  const descriptorsFromJSON = (items) => (items || []).map((d) => ({ ...d, id: decode(d.id) }));
  if (typeof Credential_.parseCreationOptionsFromJSON !== 'function') {
    define(Credential_, 'parseCreationOptionsFromJSON', {
      value: (json) => ({
        ...json,
        challenge: decode(json.challenge),
        user: { ...json.user, id: decode(json.user.id) },
        excludeCredentials: descriptorsFromJSON(json.excludeCredentials),
      }),
      writable: true,
      configurable: true,
    });
  }
  if (typeof Credential_.parseRequestOptionsFromJSON !== 'function') {
    define(Credential_, 'parseRequestOptionsFromJSON', {
      value: (json) => ({
        ...json,
        challenge: decode(json.challenge),
        allowCredentials: descriptorsFromJSON(json.allowCredentials),
      }),
      writable: true,
      configurable: true,
    });
  }
})();
