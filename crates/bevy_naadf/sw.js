// Service worker for the bevy-naadf web build.
//
// `__VERSION__` is replaced with the commit hash by `scripts/patch-wasm-loading.sh`
// during the deploy CI. Registered only by the production `init.js` (the
// CI-patched loader) — `trunk serve` never installs it.
const CACHE_NAME = 'bevy-naadf-__VERSION__';
const R2_WASM = 'https://bevy-naadf-assets.yura415.workers.dev/bevy-naadf.wasm';

// Assets to precache on install.
const PRECACHE = ['/', '/index.html'];

self.addEventListener('install', event => {
  event.waitUntil(
    caches.open(CACHE_NAME).then(cache => cache.addAll(PRECACHE))
  );
  self.skipWaiting();
});

self.addEventListener('activate', event => {
  event.waitUntil(
    caches.keys().then(keys => Promise.all(
      keys.filter(k => k.startsWith('bevy-naadf-') && k !== CACHE_NAME)
          .map(k => caches.delete(k))
    )).then(() => self.clients.claim())
  );
});

self.addEventListener('fetch', event => {
  const url = new URL(event.request.url);

  // Network-first for index.html (always get the latest deploy).
  if (url.pathname === '/' || url.pathname === '/index.html') {
    event.respondWith(networkFirst(event.request));
    return;
  }

  // Cache-first for immutable assets (content-hashed JS/wasm + shaders).
  if (url.pathname.match(/\.(js|wasm)$/) || url.pathname.startsWith('/src/assets/')) {
    event.respondWith(cacheFirst(event.request));
    return;
  }

  // Cache-first for the R2-hosted wasm binary.
  if (url.origin === 'https://bevy-naadf-assets.yura415.workers.dev') {
    event.respondWith(cacheFirst(event.request));
    return;
  }

  // Default: network only.
  event.respondWith(fetch(event.request));
});

async function networkFirst(request) {
  try {
    const response = await fetch(request);
    if (response.ok) {
      const cache = await caches.open(CACHE_NAME);
      cache.put(request, response.clone());
    }
    return response;
  } catch {
    const cached = await caches.match(request);
    if (cached) return cached;
    throw new Error('Network unavailable and no cache');
  }
}

async function cacheFirst(request) {
  const cached = await caches.match(request);
  if (cached) return cached;

  const response = await fetch(request, { cache: 'no-store' });
  if (response.ok) {
    const cache = await caches.open(CACHE_NAME);
    cache.put(request, response.clone());
  }
  return response;
}
