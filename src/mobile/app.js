'use strict';
const $ = id => document.getElementById(id);
let db, device, manifest, current = -1, audioURL, lastSaved = 0, downloading = false, syncing = false, downloadedCount = 0;
const audio = $('audio');
const say = message => { $('status').textContent = message; };
const fail = error => say(error.message || String(error));
const key = track => `${manifest.scope}:${track.id}:${track.version}`;
const progressKey = track => `${manifest.scope}:${track.id}`;
function openDB() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open('gnune-phone', 1);
    request.onupgradeneeded = () => { for (const name of ['settings', 'audio', 'progress', 'pending']) request.result.createObjectStore(name); };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}
function read(store, id) {
  return new Promise((resolve, reject) => {
    const request = db.transaction(store).objectStore(store).get(id);
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}
function write(store, id, value) {
  return new Promise((resolve, reject) => {
    const tx = db.transaction(store, 'readwrite');
    tx.objectStore(store).put(value, id);
    tx.oncomplete = resolve;
    tx.onerror = tx.onabort = () => reject(tx.error || new Error('Storage transaction failed.'));
  });
}
function all(store) {
  return new Promise((resolve, reject) => {
    const request = db.transaction(store).objectStore(store).getAll();
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}
async function api(path, options = {}, credential = device) {
  const headers = new Headers(options.headers);
  if (credential) headers.set('Authorization', `Bearer ${credential.token}`);
  if (options.body) headers.set('Content-Type', 'application/json');
  const response = await fetch(path, {signal: AbortSignal.timeout(path.startsWith('/api/media/') ? 180000 : 20000), ...options, headers, cache: 'no-store'});
  if (!response.ok) throw new Error(await response.text() || `Request failed (${response.status}).`);
  return response;
}
async function render() {
  $('pairing').hidden = Boolean(device);
  $('library').hidden = !manifest;
  if (!manifest) return;
  const list = $('tracks'); list.replaceChildren();
  let ready = 0;
  for (const [index, track] of manifest.tracks.entries()) {
    const available = Boolean(await read('audio', key(track)));
    ready += Number(available);
    const row = document.createElement('li'), button = document.createElement('button');
    const title = document.createElement('strong'), details = document.createElement('small'), status = document.createElement('span');
    title.textContent = track.title; details.textContent = `${track.artist} · ${track.album}`;
    status.textContent = available ? 'offline' : 'not downloaded';
    button.append(title, details); button.onclick = () => play(index).catch(fail); row.append(button, status); list.append(row);
  }
  downloadedCount = ready; await updateSummary();
}
async function updateSummary() {
  const pending = (await all('pending')).length;
  if (manifest) $('summary').textContent = `${downloadedCount}/${manifest.tracks.length} downloaded · ${pending} progress updates waiting`;
}
async function refresh() {
  const response = await api('/api/manifest');
  const next = await response.json();
  await write('settings', 'manifest', next); manifest = next;
  await render();
}
async function pair() {
  let code = $('pair-code').value.trim();
  if (code.startsWith('https://')) code = new URLSearchParams(new URL(code).hash.slice(1)).get('pair') || '';
  if (!/^[a-f0-9]{48}$/.test(code)) throw new Error('Paste the 48-character pairing code from Gnune on your desktop.');
  await saveProgress(); audio.pause(); current = -1; $('player').hidden = true;
  const response = await api('/api/pair', {method: 'POST', body: JSON.stringify({code, name: $('device-name').value.trim() || 'Phone'})}, null);
  device = await response.json();
  await write('settings', 'device', device);
  $('pair-code').value = ''; history.replaceState(null, '', '/');
  await refresh(); say('Paired. Download your queue while connected to your desktop.');
}
async function download() {
  if (downloading || !manifest) return;
  downloading = true; $('download').disabled = true;
  try {
    if (navigator.storage?.persist) await navigator.storage.persist().catch(() => false);
    for (const [index, track] of manifest.tracks.entries()) {
      if (await read('audio', key(track))) continue;
      say(`Downloading ${index + 1}/${manifest.tracks.length}: ${track.title}. Keep Gnune open.`);
      const response = await api(`/api/media/${encodeURIComponent(track.id)}`);
      const blob = await response.blob();
      if (!blob.size || blob.size !== track.bytes) throw new Error(`Incomplete download: ${track.title}. Retry download.`);
      await write('audio', key(track), blob);
    }
    say('Queue downloaded. Test playback in airplane mode before leaving.');
  } finally { downloading = false; $('download').disabled = false; await render(); }
}
async function saveProgress() {
  if (current < 0 || !device || !Number.isFinite(audio.currentTime)) return;
  const track = manifest.tracks[current], id = progressKey(track), completed = audio.ended, position = completed ? 0 : audio.currentTime, authToken = device.token;
  await new Promise((resolve, reject) => {
    const tx = db.transaction(['settings', 'progress', 'pending'], 'readwrite');
    const settings = tx.objectStore('settings'), request = settings.get(`sequence:${authToken}`);
    request.onsuccess = () => {
      const seq = (request.result || 0) + 1;
      settings.put(seq, `sequence:${authToken}`);
      tx.objectStore('progress').put({position, completed}, id);
      tx.objectStore('pending').put({key: id, token: authToken, seq, id: track.id, position, completed}, id);
    };
    tx.oncomplete = resolve;
    tx.onerror = tx.onabort = () => reject(tx.error || new Error('Could not save listening progress.'));
  });
  await updateSummary();
}
async function sync() {
  if (syncing) return;
  syncing = true; $('sync').disabled = true;
  try {
    await saveProgress();
    const updates = await all('pending'), groups = new Map();
    for (const update of updates) {
      if (!groups.has(update.token)) groups.set(update.token, []);
      groups.get(update.token).push(update);
    }
    for (const [token, records] of groups) {
      records.sort((a, b) => a.seq - b.seq);
      for (let start = 0; start < records.length; start += 100) {
        const batch = records.slice(start, start + 100);
        const response = await api('/api/progress', {method: 'POST', body: JSON.stringify({updates: batch.map(({seq, id, position, completed}) => ({seq, id, position, completed}))})}, {token});
        const ack = await response.json();
        await new Promise((resolve, reject) => {
          const tx = db.transaction('pending', 'readwrite'), store = tx.objectStore('pending');
          for (const sent of batch) {
            const request = store.get(sent.key);
            request.onsuccess = () => { if (request.result?.seq === sent.seq && request.result?.token === token && ack.ack.includes(sent.seq)) store.delete(sent.key); };
          }
          tx.oncomplete = resolve; tx.onerror = tx.onabort = () => reject(tx.error);
        });
      }
    }
    say('Listening progress returned to your desktop.');
  } finally { syncing = false; $('sync').disabled = false; await render(); }
}
async function play(index) {
  if (!manifest || index < 0 || index >= manifest.tracks.length) return;
  await saveProgress();
  audio.pause(); const track = manifest.tracks[index];
  let blob = await read('audio', key(track));
  if (!blob) {
    say(`Loading ${track.title} from your desktop…`);
    blob = await (await api(`/api/media/${encodeURIComponent(track.id)}`)).blob();
  }
  current = index;
  if (audioURL) URL.revokeObjectURL(audioURL);
  const progress = await read('progress', progressKey(track));
  audio.onloadedmetadata = () => { if (progress?.position > 0 && progress.position < audio.duration - 1) audio.currentTime = progress.position; };
  audioURL = URL.createObjectURL(blob); audio.src = audioURL;
  $('player').hidden = false; $('now-playing').textContent = `${track.title} — ${track.artist}`;
  await audio.play(); say(`Playing ${track.title}. Progress is saved on this phone.`);
}
async function forget() {
  if (downloading) throw new Error('Wait for the download to finish first.');
  if (!confirm('Remove all downloaded audio from this browser? Listening progress will be kept.')) return;
  audio.pause();
  await new Promise((resolve, reject) => { const tx = db.transaction('audio', 'readwrite'); tx.objectStore('audio').clear(); tx.oncomplete = resolve; tx.onerror = () => reject(tx.error); });
  await render(); say('Downloaded audio removed.');
}
async function unpair() {
  if (downloading) throw new Error('Wait for the download to finish first.');
  if (!confirm('Return pending progress, revoke this phone’s current desktop pairing, and remove its downloads? Your desktop must be reachable.')) return;
  await saveProgress(); audio.pause(); current = -1;
  await sync(); await api('/api/forget', {method: 'POST'});
  await new Promise((resolve, reject) => {
    const tx = db.transaction(['settings', 'audio'], 'readwrite');
    tx.objectStore('settings').delete('device'); tx.objectStore('settings').delete('manifest'); tx.objectStore('audio').clear();
    tx.oncomplete = resolve; tx.onerror = tx.onabort = () => reject(tx.error);
  });
  device = undefined; manifest = undefined; $('player').hidden = true;
  await render(); say('This pairing has been revoked on your desktop.');
}
audio.addEventListener('timeupdate', () => { if (Date.now() - lastSaved > 5000) { lastSaved = Date.now(); saveProgress().catch(fail); } });
audio.addEventListener('pause', () => saveProgress().catch(fail));
audio.addEventListener('seeked', () => saveProgress().catch(fail));
audio.addEventListener('ended', () => { saveProgress().then(() => play(current + 1)).catch(fail); });
audio.addEventListener('error', () => say('This audio format could not play. Try an MP3, AAC, or FLAC original supported by your phone.'));
document.addEventListener('visibilitychange', () => { if (document.hidden) saveProgress().catch(fail); else if (device) sync().catch(() => say('Desktop unavailable. Progress remains saved on this phone.')); });
window.addEventListener('online', () => { if (device) sync().catch(() => say('Reconnect to your desktop Wi-Fi and tap Sync progress.')); });
for (const [id, action] of [['pair', pair], ['download', download], ['sync', sync], ['forget', forget], ['unpair', unpair], ['previous', () => play(current - 1)], ['next', () => play(current + 1)]]) $(id).onclick = () => action().catch(fail);
(async () => {
  if (!isSecureContext || !('serviceWorker' in navigator)) throw new Error('Offline Gnune needs a trusted HTTPS connection. Check the desktop certificate and this phone’s certificate trust settings.');
  db = await openDB();
  await navigator.serviceWorker.register('/sw.js'); await navigator.serviceWorker.ready;
  device = await read('settings', 'device'); manifest = await read('settings', 'manifest');
  const code = new URLSearchParams(location.hash.slice(1)).get('pair');
  if (code) { $('pair-code').value = code; $('pairing').hidden = false; history.replaceState(null, '', '/'); }
  await render();
  if (code) { $('pairing').hidden = false; say('Pairing link ready. Install Gnune first, or pair in this browser.'); }
  else if (device) {
    say('Your saved queue is ready. Connect to your desktop Wi-Fi to sync.');
    try { await refresh(); await sync(); } catch (_) { say('Working offline. Downloaded tracks and progress are saved on this phone.'); }
  } else say('Install Gnune, then paste your desktop pairing code.');
})().catch(fail);
