// The web remote's notifications, in Node: when the page offers them, the subscription it hands
// the computer, the note the service worker shows and where tapping it goes.
//
//   node script/site/test/run.mjs

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { madeWith, notificationFor, openTarget, openUrl, pushSupport, serverKey, subscriptionFor } from '../../../site/remote/js/push.js';
import { ROOT } from '../serve.mjs';

// RFC 8291's example keys (section 5): a sender's public key and a receiver's.
const KEY = 'BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8';
const UA = 'BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4';

const env = (o) => ({ push: true, worker: true, notification: true, ios: false, standalone: false, permission: 'default', ...o });

test('an iPhone’s Safari is told to add Trek to the Home Screen; elsewhere it depends on the browser', () => {
  assert.equal(pushSupport(env({ ios: true })), 'install');
  assert.equal(pushSupport(env({ ios: true, push: false })), 'install', 'Safari has no PushManager outside a Home Screen app');
  assert.equal(pushSupport(env({ ios: true, standalone: true })), 'ready');
  assert.equal(pushSupport(env({})), 'ready', 'Chrome on Android, in a tab');
  assert.equal(pushSupport(env({ permission: 'denied' })), 'denied');
  assert.equal(pushSupport(env({ push: false })), 'unsupported');
  assert.equal(pushSupport(env({ worker: false })), 'unsupported');
});

test('the computer’s key is 65 bytes, uncompressed; anything else is refused', () => {
  const bytes = serverKey(KEY);
  assert.equal(bytes.length, 65);
  assert.equal(bytes[0], 4);
  assert.throws(() => serverKey('BTBZMqHH6r4Tts7J_aSIgg'));
  assert.throws(() => serverKey(''));
});

test('the subscription goes to the computer as endpoint and keys, and only an https one', () => {
  const sub = { toJSON: () => ({ endpoint: 'https://web.push.apple.com/QGuQ', expirationTime: null, keys: { p256dh: UA, auth: 'BTBZMqHH6r4Tts7J_aSIgg' } }) };
  assert.deepEqual(subscriptionFor(sub), { endpoint: 'https://web.push.apple.com/QGuQ', p256dh: UA, auth: 'BTBZMqHH6r4Tts7J_aSIgg' });
  assert.throws(() => subscriptionFor({ endpoint: 'http://push.example/x', keys: { p256dh: UA, auth: 'x' } }));
  assert.throws(() => subscriptionFor({ endpoint: 'https://push.example/x', keys: {} }));
  assert.throws(() => subscriptionFor(null));
});

test('a subscription made with another computer’s key isn’t this one’s', () => {
  const key = serverKey(KEY);
  assert.ok(madeWith({ options: { applicationServerKey: key.buffer } }, KEY));
  const other = key.slice();
  other[10] ^= 1;
  assert.ok(!madeWith({ options: { applicationServerKey: other.buffer } }, KEY));
  assert.ok(!madeWith({ options: {} }, KEY));
});

test('a note shows what the computer said, one per thread, and opens it', () => {
  const shown = notificationFor({ title: 'Trek · api', body: 'Needs your approval: Fix the bug', thread: 't1', host: 'm1', tag: 'm1:t1' });
  assert.equal(shown.title, 'Trek · api');
  assert.equal(shown.options.body, 'Needs your approval: Fix the bug');
  assert.equal(shown.options.tag, 'm1:t1');
  assert.deepEqual(shown.options.data, { thread: 't1', host: 'm1' });
  // Something is always shown: a browser drops the permission of a page whose push showed nothing.
  assert.equal(notificationFor(null).title, 'Trek');
  assert.equal(notificationFor({ title: 42 }).title, 'Trek');
  const url = openUrl(shown.options.data);
  assert.equal(url, '/remote/#t=t1&h=m1');
  assert.deepEqual(openTarget(url.slice(url.indexOf('#'))), { thread: 't1', host: 'm1' });
  assert.deepEqual(openTarget('#t=abc'), { thread: 'abc', host: null }, 'links from before name the thread alone');
  assert.equal(openTarget('#p=xyz'), null, 'a pairing link isn’t one');
  assert.equal(openUrl({}), '/remote/');
});

test('the service worker shows every push and opens its thread, as push.js reads it', () => {
  const sw = readFileSync(join(ROOT, 'remote', 'sw.js'), 'utf8');
  assert.match(sw, /addEventListener\('push'/);
  assert.match(sw, /showNotification\(/);
  assert.match(sw, /addEventListener\('notificationclick'/);
  assert.match(sw, /openWindow\(url\)/);
  assert.match(sw, /\/remote\/#\$\{hash\}/, 'the same address as openUrl');
});
