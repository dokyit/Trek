// The web remote's service worker: the page and its files from the network when it's there (so
// a new version shows the next time it opens), and from what was kept when it isn't, so the
// remote opens without a connection and says so. It keeps nothing else: what Trek says comes over
// WebTransport, which this doesn't see.
//
// And notifications: the computer sends them itself, through the browser's push service, encrypted
// to this browser (Web Push; `js/push.js` subscribes). Each is shown as it comes (a browser drops
// a page's push permission if one isn't), and tapping it opens its thread. What a note says is
// read as in `js/push.js` (`notificationFor`, `openUrl`): a classic worker can't import it.

const CACHE = 'trek-remote-v2';
const SHELL = [
  '/remote/',
  '/remote/css/git.css',
  '/remote/css/mac.css',
  '/remote/css/media.css',
  '/remote/css/notes.css',
  '/remote/css/thread-extras.css',
  '/remote/css/viz.css',
  '/remote/js/boot.js',
  '/remote/js/client.js',
  '/remote/js/demo.js',
  '/remote/js/demo/git.js',
  '/remote/js/demo/index.js',
  '/remote/js/demo/mac.js',
  '/remote/js/demo/media.js',
  '/remote/js/demo/notes.js',
  '/remote/js/demo/thread.js',
  '/remote/js/features/git.js',
  '/remote/js/features/index.js',
  '/remote/js/features/look-mac.js',
  '/remote/js/features/mac-basecamp.js',
  '/remote/js/features/mac-parts.js',
  '/remote/js/features/mac-rules.js',
  '/remote/js/features/mac-search.js',
  '/remote/js/features/mac-settings.js',
  '/remote/js/features/mac-text.js',
  '/remote/js/features/mac-usage.js',
  '/remote/js/features/mac.js',
  '/remote/js/features/media.js',
  '/remote/js/features/notes.js',
  '/remote/js/features/notify.js',
  '/remote/js/features/thread-extras.js',
  '/remote/js/features/turn-actions.js',
  '/remote/js/features/viz-card.js',
  '/remote/js/features/viz-charts.js',
  '/remote/js/features/viz-diagrams.js',
  '/remote/js/features/viz-dom.js',
  '/remote/js/features/viz-math.js',
  '/remote/js/features/viz-mockup.js',
  '/remote/js/features/viz-spec.js',
  '/remote/js/features/viz.js',
  '/remote/js/frame.js',
  '/remote/js/items.js',
  '/remote/js/link.js',
  '/remote/js/look.js',
  '/remote/js/main.js',
  '/remote/js/markdown.js',
  '/remote/js/model.js',
  '/remote/js/push.js',
  '/remote/js/routes.js',
  '/remote/js/screens.js',
  '/remote/js/store.js',
  '/remote/js/thread.js',
  '/remote/js/ui.js',
  '/remote/js/wire.js',
  '/remote/manifest.webmanifest',
  '/remote/remote.css',
  '/remote/vendor/hooks.mjs',
  '/remote/vendor/htm.mjs',
  '/remote/vendor/katex/katex.min.css',
  '/remote/vendor/katex/katex.mjs',
  '/remote/vendor/marked.mjs',
  '/remote/vendor/preact.mjs',
  '/remote/vendor/purify.mjs',
  '/favicon.svg',
  '/assets/fonts/JetBrainsMono-Regular.woff2',
];

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((cache) => cache.addAll(SHELL))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener('fetch', (event) => {
  const request = event.request;
  if (request.method !== 'GET') return;
  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;
  event.respondWith(
    fetch(request)
      .then((response) => {
        if (response.ok && (url.pathname.startsWith('/remote/') || SHELL.includes(url.pathname))) {
          const copy = response.clone();
          caches.open(CACHE).then((cache) => cache.put(request, copy));
        }
        return response;
      })
      .catch(() => caches.match(request, { ignoreSearch: true }).then((hit) => hit || (request.mode === 'navigate' ? caches.match('/remote/') : Response.error()))),
  );
});

self.addEventListener('push', (event) => {
  let note = {};
  try {
    note = event.data ? event.data.json() : {};
  } catch {
    // Shown plainly: every push shows something.
  }
  const title = typeof note.title === 'string' && note.title ? note.title : 'Trek';
  const data = { thread: typeof note.thread === 'string' ? note.thread : null, host: typeof note.host === 'string' ? note.host : null };
  event.waitUntil(
    self.registration.showNotification(title, {
      body: typeof note.body === 'string' ? note.body : '',
      tag: typeof note.tag === 'string' && note.tag ? note.tag : data.host || 'trek',
      icon: '/assets/img/icon-192.png',
      badge: '/assets/img/icon-192.png',
      data,
    }),
  );
});

self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const data = event.notification.data || {};
  const params = new URLSearchParams();
  if (data.thread) params.set('t', data.thread);
  if (data.host) params.set('h', data.host);
  const hash = params.toString();
  const url = hash ? `/remote/#${hash}` : '/remote/';
  event.waitUntil(
    self.clients.matchAll({ type: 'window', includeUncontrolled: true }).then((windows) => {
      // The remote open already: it goes to the thread itself, and comes to the front.
      const open = windows.find((w) => new URL(w.url).pathname.startsWith('/remote/'));
      if (open) {
        open.postMessage({ type: 'open', thread: data.thread || null, host: data.host || null });
        return open.focus();
      }
      return self.clients.openWindow(url);
    }),
  );
});
