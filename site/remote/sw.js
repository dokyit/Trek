// The web remote's service worker: the page and its files from the network when it's there (so
// a new version shows the next time it opens), and from what was kept when it isn't, so the
// remote opens without a connection and says so. It keeps nothing else: what Trek says comes over
// WebTransport, which this doesn't see.

const CACHE = 'trek-remote-v1';
const SHELL = [
  '/remote/',
  '/remote/remote.css',
  '/remote/manifest.webmanifest',
  '/remote/js/boot.js',
  '/remote/js/main.js',
  '/remote/js/model.js',
  '/remote/js/client.js',
  '/remote/js/wire.js',
  '/remote/js/frame.js',
  '/remote/js/link.js',
  '/remote/js/store.js',
  '/remote/js/ui.js',
  '/remote/js/markdown.js',
  '/remote/js/items.js',
  '/remote/js/thread.js',
  '/remote/js/screens.js',
  '/remote/js/look.js',
  '/remote/js/demo.js',
  '/remote/vendor/preact.mjs',
  '/remote/vendor/hooks.mjs',
  '/remote/vendor/htm.mjs',
  '/remote/vendor/marked.mjs',
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
