'use strict';
const SHELL = 'gnune-shell-v7';
const FILES = ['/', '/catalog.js', '/app.js', '/style.css', '/manifest.webmanifest', '/icon.svg', '/icon-192.png', '/icon-512.png'];
self.addEventListener('install', event => event.waitUntil(caches.open(SHELL).then(cache => cache.addAll(FILES))));
self.addEventListener('activate', event => event.waitUntil((async () => {
  for (const name of await caches.keys()) if (name.startsWith('gnune-shell-') && name !== SHELL) await caches.delete(name);
  // Activate after existing clients close. Never mix an old page with new scripts
  // by taking over an in-flight load; the complete shell is updated atomically.
})()));
self.addEventListener('fetch', event => {
  if (event.request.method !== 'GET' || new URL(event.request.url).origin !== self.location.origin) return;
  const path = new URL(event.request.url).pathname;
  if (event.request.mode === 'navigate') {
    event.respondWith(caches.match('/').then(cached => cached || fetch(event.request)));
  } else if (FILES.includes(path)) {
    event.respondWith(caches.match(path).then(cached => cached || fetch(event.request)));
  }
  // API data and media never enter the shell cache. Complete audio Blobs live
  // in IndexedDB and are played as object URLs, including while offline.
});
