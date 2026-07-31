// Service worker — the shell cache, and the share-sheet handoff.
//
// Two jobs, and deliberately no third: this worker never touches an upload, a
// download or an /api call. Those need the session cookie and the desktop's
// answer, and a cached "yes" to a question about which files are being shared
// right now is worse than no answer at all.
//
//   1. Cache the shell so the app opens from the home screen without the
//      desktop being awake. It opens on the PIN screen and says it cannot
//      reach the desktop, which is the honest thing for it to say.
//   2. Catch the POST the OS share sheet makes at /share, park the files, and
//      hand them to the page. The page then uploads them the normal way, with
//      the normal cookie and the normal progress bars.
//
// The share POST cannot go to the network: the session cookie is SameSite=
// Strict, and a navigation the operating system starts carries no cookie at
// all, so the desktop would answer 401 and the files would be gone. Catching it
// here keeps the request from ever leaving the phone.

const SHELL = 'tulipix-shell-v1';
const HANDOFF = 'tulipix-share-v1';

// Everything the app needs to paint itself. Not /api/*, not /dl/*, not
// /upload/* — those are the live conversation with the desktop.
const SHELL_URLS = [
  '/',
  '/app.css',
  '/app.js',
  '/logo.png',
  '/icon-192.png',
  '/manifest.webmanifest',
];

self.addEventListener('install', (e) => {
  e.waitUntil(caches.open(SHELL).then((c) => c.addAll(SHELL_URLS)).then(() => self.skipWaiting()));
});

self.addEventListener('activate', (e) => {
  e.waitUntil(
    caches
      .keys()
      .then((names) =>
        Promise.all(names.filter((n) => n !== SHELL && n !== HANDOFF).map((n) => caches.delete(n))),
      )
      .then(() => self.clients.claim()),
  );
});

// The shared files, parked where the page can pick them up. Cache Storage
// rather than IndexedDB because a Response holds a Blob without copying it
// through a structured clone, and a shared video can be gigabytes.
async function park(request) {
  const form = await request.formData();
  const files = form.getAll('files').filter((f) => f && f.name !== undefined);

  // Shared text with no file — a link, usually. Nothing to send, so say so
  // rather than opening an empty upload screen.
  if (files.length === 0) return Response.redirect('/?share=empty', 303);

  const box = await caches.open(HANDOFF);
  const names = [];
  for (let i = 0; i < files.length; i += 1) {
    const file = files[i];
    const key = `/__share/${Date.now()}-${i}`;
    await box.put(
      new Request(key),
      new Response(file, {
        headers: {
          'content-type': file.type || 'application/octet-stream',
          // The name does not survive the Blob, so it rides along in a header.
          'x-name': encodeURIComponent(file.name || `shared-${i}`),
        },
      }),
    );
    names.push(key);
  }
  return Response.redirect(`/?share=${names.length}`, 303);
}

// Network first, cache only as the fallback. The desktop's copy always wins
// while it is reachable, so an app.js that changed in a Tulipix upgrade lands
// without the worker having to notice anything.
function freshen(request, key) {
  return fetch(request)
    .then((res) => {
      // Only a clean URL updates the cache. A ?k= response carries a Set-Cookie
      // for a single-use pairing key, and neither half of that belongs in
      // storage that outlives the request.
      if (res.ok && !new URL(request.url).search) {
        const copy = res.clone();
        caches.open(SHELL).then((c) => c.put(key || request, copy));
      }
      return res;
    })
    .catch(() => caches.match(key || request).then((hit) => hit || Response.error()));
}

self.addEventListener('fetch', (e) => {
  const url = new URL(e.request.url);

  if (e.request.method === 'POST' && url.pathname === '/share') {
    e.respondWith(park(e.request));
    return;
  }

  if (e.request.method !== 'GET') return;

  // Opening the app. Always cached under bare '/' whatever the query string is
  // — the two that turn up, ?k= and ?share=, are single-use and caching either
  // one would be storing a spent pairing key.
  if (e.request.mode === 'navigate') {
    e.respondWith(freshen(e.request, '/'));
    return;
  }

  if (url.search || !SHELL_URLS.includes(url.pathname)) return;
  e.respondWith(freshen(e.request));
});
