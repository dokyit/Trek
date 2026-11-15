// Notifications on this phone, sent by the computer itself through the browser's push service
// (Web Push: `trek_remote::webpush` on the computer). The page asks the browser for a
// subscription with the computer's public key (`web_push` gives it), after a tap, and hands it
// to the computer on its own connection; the service worker (`sw.js`) shows what comes.
//
// Plain code: `test/remote.test.mjs` runs it in Node.

import { fromBase64 } from './link.js';

/**
 * Whether this browser can take notifications from Trek, and if not what to say:
 * `ready`, `install` (an iPhone or iPad's Safari: only a Home Screen app gets them), `denied`
 * (the user said no; only the browser's settings undo that) or `unsupported`.
 */
export function pushSupport({ push, worker, notification, ios, standalone, permission }) {
  if (ios && !standalone) return 'install';
  if (!push || !worker || !notification) return 'unsupported';
  if (permission === 'denied') return 'denied';
  return 'ready';
}

/** What this browser is, for `pushSupport`. */
export function environment(win = globalThis) {
  const nav = win.navigator || {};
  const ua = nav.userAgent || '';
  const ios = /iPhone|iPad|iPod/.test(ua) || (nav.platform === 'MacIntel' && nav.maxTouchPoints > 1);
  let standalone = nav.standalone === true;
  try {
    standalone = standalone || !!win.matchMedia?.('(display-mode: standalone)').matches;
  } catch {
    // Not one, then.
  }
  return {
    push: 'PushManager' in win,
    worker: 'serviceWorker' in nav,
    notification: 'Notification' in win,
    ios,
    standalone,
    permission: 'Notification' in win ? win.Notification.permission : 'default',
  };
}

/** The computer's key (base64url) as `applicationServerKey` takes it: 65 bytes, uncompressed. */
export function serverKey(key) {
  const bytes = fromBase64(key || '');
  if (bytes.length !== 65 || bytes[0] !== 4) throw new Error('The computer’s push key isn’t a P-256 key');
  return bytes;
}

/** A subscription as the computer takes it (`web_push.subscribe`). */
export function subscriptionFor(sub) {
  const json = typeof sub?.toJSON === 'function' ? sub.toJSON() : sub;
  const endpoint = json?.endpoint;
  const p256dh = json?.keys?.p256dh;
  const auth = json?.keys?.auth;
  if (typeof endpoint !== 'string' || !endpoint.startsWith('https://') || !p256dh || !auth) {
    throw new Error('The browser gave no usable subscription');
  }
  return { endpoint, p256dh, auth };
}

/** Whether `sub` was made with `key`: one made with another computer's key (or an old one) won't do. */
export function madeWith(sub, key) {
  const used = sub?.options?.applicationServerKey;
  if (!used) return false;
  const a = new Uint8Array(used);
  const b = serverKey(key);
  return a.length === b.length && a.every((x, i) => x === b[i]);
}

/** What the service worker shows for a push's JSON (`webpush::PushNote`). */
export function notificationFor(data) {
  const note = data && typeof data === 'object' ? data : {};
  const title = typeof note.title === 'string' && note.title ? note.title : 'Trek';
  const body = typeof note.body === 'string' ? note.body : '';
  const thread = typeof note.thread === 'string' ? note.thread : null;
  const host = typeof note.host === 'string' ? note.host : null;
  return {
    title,
    options: {
      body,
      tag: typeof note.tag === 'string' && note.tag ? note.tag : host || 'trek',
      icon: '/assets/img/icon-192.png',
      badge: '/assets/img/icon-192.png',
      data: { thread, host },
    },
  };
}

/** Where tapping a notification goes: the thread, on the computer it came from. */
export function openUrl(data) {
  const params = new URLSearchParams();
  if (data?.thread) params.set('t', data.thread);
  if (data?.host) params.set('h', data.host);
  const hash = params.toString();
  return hash ? `/remote/#${hash}` : '/remote/';
}

/** A notification's target read back from the address (`#t=…&h=…`; `#t=…` alone before). */
export function openTarget(hash) {
  const params = new URLSearchParams((hash || '').replace(/^#/, ''));
  const thread = params.get('t');
  return thread ? { thread, host: params.get('h') || null } : null;
}

/** Whether the page asks the computer about notifications: only one that offers them (`web_push`). */
export function asksComputer({ online, features }) {
  return !!online && !!features?.has?.('web_push');
}

/**
 * Which row Settings shows for this phone's notifications, or `null` for none: a computer that
 * doesn't offer them (a Trek from before them) gets no row at all, so the page is as it was.
 * `push` is the computer's answer to `web_push` (`null` until it comes); `here`, whether this
 * browser still holds the subscription.
 */
export function notifyRow({ served, support, online, push, here }) {
  if (!served) return null;
  if (support === 'install' || support === 'unsupported') return support;
  if (!push) return online ? 'asking' : 'offline';
  if (support === 'denied') return 'denied';
  const on = !!push.subscribed && !!here;
  if (on && !push.enabled) return 'off-there';
  return on ? 'on' : 'off';
}
