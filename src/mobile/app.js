'use strict';
const $ = id => document.getElementById(id);
const Catalog = globalThis.GnuneCatalog;
let db, device, manifest, catalog = Catalog.build(null), pendingPair = '', needsPairing = false;
let changingPairing = false;
let audioKeys = new Set(), artKeys = [], currentTrack, currentVideo;
let playQueue = [], queueIndex = -1, videoQueue = [], videoIndex = -1;
let lastSaved = 0, syncing = false, reconnecting = false, pendingCount = 0, showPlayer = false;
let playGeneration = 0, mediaController, shuffle = false, repeat = false;
let view = {pivot: 'artists', detail: null, offline: false, search: ''}, backStack = [], pageSize = 80;
const audio = $('audio'), video = $('video');
const artURLs = new Map(), artRequests = new Map(), artTasks = [];
let artWorkers = 0;
const say = message => { $('status').textContent = message; };
const fail = error => { if (error?.name !== 'AbortError') say(error.message || String(error)); };
const key = track => `${manifest.scope}:${track.id}:${track.version}`;
const progressKey = track => `${manifest.scope}:${track.id}`;
const available = track => audioKeys.has(key(track));
const duration = seconds => `${Math.floor((seconds || 0) / 60)}:${String(Math.floor((seconds || 0) % 60)).padStart(2, '0')}`;
function node(tag, className, text) {
  const result = document.createElement(tag);
  if (className) result.className = className;
  if (text !== undefined) result.textContent = text;
  return result;
}
function pairingCode(value) {
  let code = value.trim();
  if (code.startsWith('https://')) {
    const link = new URL(code);
    if (link.origin !== location.origin) throw new Error('Open the pairing link itself to pair with that desktop.');
    code = new URLSearchParams(link.hash.slice(1)).get('pair') || '';
  }
  if (!/^[a-f0-9]{48}$/.test(code)) throw new Error('Scan a fresh desktop QR, or enter its full pairing link or 48-character code.');
  return code;
}
function rememberPair(code) {
  pendingPair = code;
  try {
    if (code) sessionStorage.setItem('gnune-pair', code);
    else sessionStorage.removeItem('gnune-pair');
  } catch (_) { /* Pairing works even when tab storage is restricted. */ }
}
function openDB() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open('gnune-phone', 2);
    request.onupgradeneeded = () => {
      for (const name of ['settings', 'audio', 'progress', 'pending', 'art']) {
        if (!request.result.objectStoreNames.contains(name)) request.result.createObjectStore(name);
      }
    };
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
function transaction(stores, action) {
  return new Promise((resolve, reject) => {
    const tx = db.transaction(stores, 'readwrite');
    action(tx);
    tx.oncomplete = resolve;
    tx.onerror = tx.onabort = () => reject(tx.error || new Error('Phone storage is unavailable.'));
  });
}
const write = (store, id, value) => transaction(store, tx => tx.objectStore(store).put(value, id));
function all(store, keys = false) {
  return new Promise((resolve, reject) => {
    const objectStore = db.transaction(store).objectStore(store);
    const request = keys ? objectStore.getAllKeys() : objectStore.getAll();
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}
async function api(path, options = {}, credential = device) {
  const headers = new Headers(options.headers);
  if (credential) headers.set('Authorization', `Bearer ${credential.token}`);
  if (options.body) headers.set('Content-Type', 'application/json');
  const response = await fetch(path, {
    signal: AbortSignal.timeout(path.startsWith('/api/media/') ? 180000 : 20000),
    ...options, headers, cache: 'no-store', credentials: 'same-origin',
  });
  if (!response.ok) {
    const error = new Error(await response.text() || `Request failed (${response.status}).`);
    error.status = response.status;
    throw error;
  }
  return response;
}
function acceptManifest(next) {
  manifest = next;
  catalog = Catalog.build(next);
}
async function refresh() {
  if (!device) return;
  const credential = device;
  const next = await (await api('/api/manifest', {}, credential)).json();
  if (changingPairing || device?.token !== credential.token) return;
  await write('settings', 'manifest', next);
  acceptManifest(next);
  render();
}
function saveNavigation() {
  if (manifest) write('settings', 'navigation', {scope: manifest.scope, view, backStack}).catch(fail);
}
function navigate(next, remember = true) {
  if (remember) backStack.push({...view});
  backStack = backStack.slice(-20);
  view = {...view, search: '', ...next};
  pageSize = 80;
  showPlayer = false;
  saveNavigation();
  render();
  window.scrollTo?.({top: 0});
}
function back() {
  if (showPlayer) showPlayer = false;
  else view = backStack.pop() || {...view, detail: null, search: ''};
  pageSize = 80;
  saveNavigation();
  render();
}
async function updateSummary() {
  pendingCount = (await all('pending')).length;
  renderSummary();
}
function renderSummary() {
  if (!manifest) return;
  const saved = manifest.tracks.filter(available).length;
  $('summary').textContent = `${saved} songs on this phone · ${pendingCount} progress updates waiting`;
}
const currentMedia = () => currentVideo ? video : audio;
function renderPlayer() {
  const track = currentVideo || currentTrack;
  $('mini-player').hidden = !track || showPlayer;
  $('player').hidden = !track || !showPlayer;
  $('library').hidden = !manifest || showPlayer;
  if (!track) return;
  $('now-title').textContent = track.title;
  $('now-artist').textContent = track.artist || (currentVideo ? 'video' : 'Unknown artist');
  $('now-album').textContent = track.album || '';
  $('mini-title').textContent = track.title;
  $('mini-artist').textContent = track.artist || 'video';
  $('mini-toggle').textContent = currentMedia().paused ? '▶' : 'Ⅱ';
  $('mini-toggle').setAttribute('aria-label', currentMedia().paused ? 'Play current item' : 'Pause current item');
  $('audio').hidden = Boolean(currentVideo);
  $('video').hidden = !currentVideo;
  $('now-cover').hidden = Boolean(currentVideo);
  $('now-cover').replaceChildren(cover(track.art));
  $('mini-art').replaceChildren(cover(track.art));
  const count = currentVideo ? videoQueue.length : playQueue.length;
  const index = currentVideo ? videoIndex : queueIndex;
  const transferNote = currentVideo
    ? 'video streams while playing; no offline video download'
    : 'only the playing song transfers; played songs are saved when storage allows';
  $('queue-summary').textContent = `${index + 1} of ${count} · ${transferNote}`;
  $('shuffle').hidden = Boolean(currentVideo);
  $('repeat').setAttribute('aria-pressed', String(repeat));
  $('shuffle').setAttribute('aria-pressed', String(shuffle));
}
function render() {
  $('pairing').hidden = Boolean(device) && !pendingPair && !needsPairing && !changingPairing;
  $('replace-help').hidden = !device || (!pendingPair && !changingPairing);
  $('cancel-pair').hidden = !device || (!pendingPair && !changingPairing);
  $('manual-pairing').open = !pendingPair;
  $('pair-help').textContent = changingPairing
    ? 'Paste a fresh desktop pairing link below. Your current pairing stays active until this succeeds.'
    : pendingPair ? 'Your link is ready. Tap Pair phone.' : 'Scan the QR in desktop Devices or enter its link below.';
  $('scope-label').textContent = manifest?.mode === 'library' ? 'your desktop collection' : 'shared desktop queue';
  $('connection').textContent = device ? 'paired to your desktop' : 'on your phone';
  $('settings').hidden = !device;
  for (const button of document.querySelectorAll('[data-pivot]')) {
    button.setAttribute('aria-current', button.dataset.pivot === view.pivot ? 'page' : 'false');
    if (button.dataset.pivot.startsWith('video')) button.hidden = !manifest?.includes_video;
  }
  $('offline-filter').setAttribute('aria-pressed', String(view.offline));
  $('search').value = view.search;
  renderPlayer();
  if (!manifest) return;
  renderBrowse();
  renderSummary();
}
function visible() { return Catalog.visible(catalog, view, available, currentVideo ? videoQueue : playQueue); }
function trackButton(track, tracks, index, isVideo = false) {
  const row = node('li', 'browse-row');
  if ((currentVideo || currentTrack)?.id === track.id) row.classList.add('playing-row');
  const button = node('button', 'item');
  button.append(cover(track.art));
  const text = node('span', 'item-text');
  text.append(node('strong', '', track.title), node('small', '', `${track.artist || ''}${track.album ? ` · ${track.album}` : ''} · ${duration(track.duration)}`));
  if (!isVideo && available(track)) text.append(node('span', 'saved-marker', 'on phone'));
  button.append(text);
  button.onclick = () => (isVideo ? playVideo(index, tracks.map(track => track.id)) : play(index, tracks.map(track => track.id))).catch(fail);
  row.append(button);
  if (!isVideo) {
    const add = node('button', 'row-action', '+');
    add.setAttribute('aria-label', `Add ${track.title} to up next`);
    add.onclick = () => addQueue([track.id]);
    row.append(add);
  }
  return row;
}
let pageObserver;
function renderBrowse() {
  const result = visible();
  $('view-title').textContent = result.title;
  $('view-subtitle').textContent = result.subtitle;
  $('back').hidden = !view.detail && backStack.length === 0;
  const playable = ['tracks', 'videos'].includes(result.kind);
  $('view-actions').hidden = !playable || result.items.length === 0;
  $('queue-view').hidden = result.kind === 'videos';
  $('shuffle-view').hidden = result.kind === 'videos';
  const target = $('browse');
  target.replaceChildren();
  const list = node(result.kind === 'albums' ? 'div' : 'ol', result.kind === 'albums' ? 'album-grid' : 'browse-list');
  const items = result.items.slice(0, pageSize);
  let letter = '';
  for (const [index, item] of items.entries()) {
    if (result.kind === 'tracks' || result.kind === 'videos') {
      list.append(trackButton(item, result.items, index, result.kind === 'videos'));
    } else if (result.kind === 'albums') {
      const button = node('button', 'album-tile');
      button.append(cover(item.art), node('strong', '', item.title), node('small', '', item.artist));
      button.onclick = () => navigate({detail: {type: 'album', id: item.id, artist: view.detail?.type === 'artist' ? view.detail.id : undefined}});
      list.append(button);
    } else {
      const first = item.title.slice(0, 1).toLocaleLowerCase();
      if (result.kind === 'artists' && first !== letter) {
        list.append(node('li', 'letter-heading', first));
        letter = first;
      }
      const row = node('li', 'browse-row'), button = node('button', 'item');
      const text = node('span', 'item-text');
      const description = result.kind === 'artists'
        ? `${item.albums.size} albums · ${item.tracks.length} songs`
        : `${item.tracks.length} ${result.kind === 'video-playlists' ? 'videos' : 'songs'}`;
      text.append(node('strong', '', item.title), node('small', '', description));
      button.append(text);
      button.onclick = () => navigate({detail: {type: result.kind === 'artists' ? 'artist' : result.kind === 'video-playlists' ? 'video-playlist' : 'playlist', id: item.id}});
      row.append(button);
      list.append(row);
    }
  }
  if (!result.items.length) target.append(node('p', 'empty', view.offline ? 'No saved songs in this view. Play a song on Wi-Fi to save it here.' : 'No items in this view.'));
  else target.append(list);
  $('more').hidden = result.items.length <= pageSize;
  pageObserver?.disconnect();
  if (!$('more').hidden && 'IntersectionObserver' in globalThis) {
    pageObserver = new IntersectionObserver(entries => {
      if (entries.some(entry => entry.isIntersecting)) { pageSize += 80; renderBrowse(); }
    }, {rootMargin: '200px'});
    pageObserver.observe($('more'));
  }
}
function cover(id) {
  const wrapper = node('span', 'cover');
  wrapper.append(node('span', 'placeholder', '♪'));
  if (!id || !manifest || !device) return wrapper;
  const image = node('img');
  image.alt = '';
  image.loading = 'lazy';
  wrapper.append(image);
  const load = () => getArt(id).then(url => { if (url) image.src = url; }).catch(() => {});
  if ('IntersectionObserver' in globalThis) {
    const observer = new IntersectionObserver(entries => {
      if (entries.some(entry => entry.isIntersecting)) { observer.disconnect(); load(); }
    }, {rootMargin: '100px'});
    observer.observe(wrapper);
  } else load();
  return wrapper;
}
function getArt(id) {
  const scope = manifest.scope, credential = device, cacheKey = `${scope}:${id}`;
  if (artURLs.has(cacheKey)) return Promise.resolve(artURLs.get(cacheKey));
  if (artRequests.has(cacheKey)) return artRequests.get(cacheKey);
  const result = new Promise(resolve => { artTasks.push({id, scope, credential, cacheKey, resolve}); pumpArt(); });
  artRequests.set(cacheKey, result);
  return result;
}
async function pumpArt() {
  if (artWorkers >= 2 || !artTasks.length) return;
  artWorkers++;
  const task = artTasks.shift();
  try {
    let blob = await read('art', task.cacheKey);
    if (!blob) {
      blob = await (await api(`/api/art/${encodeURIComponent(task.id)}`, {}, task.credential)).blob();
      if (blob.size > 2 * 1024 * 1024) throw new Error('Artwork too large');
      await transaction('art', tx => {
        const store = tx.objectStore('art');
        while (artKeys.length >= 64) store.delete(artKeys.shift());
        store.put(blob, task.cacheKey);
      }).catch(() => {});
      artKeys.push(task.cacheKey);
    }
    const url = URL.createObjectURL(blob);
    while (artURLs.size >= 64) {
      const [old, url] = artURLs.entries().next().value;
      URL.revokeObjectURL(url); artURLs.delete(old);
    }
    artURLs.set(task.cacheKey, url);
    task.resolve(url);
  } catch (_) { task.resolve(null); }
  finally { artRequests.delete(task.cacheKey); artWorkers--; pumpArt(); }
}
async function pair() {
  const code = pairingCode($('pair-code').value);
  $('pair').disabled = true;
  try {
    await saveProgress();
    const nextDevice = await (await api('/api/pair', {method: 'POST', body: JSON.stringify({code, name: $('device-name').value.trim() || 'Phone'})}, null)).json();
    await transaction('settings', tx => {
      tx.objectStore('settings').put(nextDevice, 'device');
      tx.objectStore('settings').delete('manifest');
      tx.objectStore('settings').delete('playback');
    });
    stopPlayback();
    device = nextDevice; needsPairing = false;
    changingPairing = false;
    acceptManifest(undefined);
    rememberPair(''); $('pair-code').value = '';
    view = {pivot: nextDevice.mode === 'library' ? 'artists' : 'songs', detail: null, offline: false, search: ''};
    backStack = [];
    render();
    await refresh();
    say('Paired. Tap a song to play. Songs transfer only as they play.');
  } finally { $('pair').disabled = false; }
}
function stopPlayback() {
  playGeneration++;
  mediaController?.abort();
  audio.pause(); video.pause();
  if (audio.src?.startsWith('blob:')) URL.revokeObjectURL(audio.src);
  audio.removeAttribute('src'); audio.load();
  video.removeAttribute('src'); video.load();
  currentTrack = undefined; currentVideo = undefined;
  playQueue = []; videoQueue = []; queueIndex = -1; videoIndex = -1;
  showPlayer = false;
}
function saveQueue() {
  return write('settings', 'playback', {scope: manifest.scope, tracks: playQueue, index: queueIndex});
}
function addQueue(ids) {
  playQueue.push(...ids);
  saveQueue().catch(fail);
  say(`Added ${ids.length === 1 ? 'song' : `${ids.length} songs`} to up next. No audio downloaded.`);
  if (view.detail?.type === 'queue') renderBrowse();
}
async function play(index, queue = playQueue) {
  if (!manifest || index < 0 || index >= queue.length) return;
  const track = catalog.tracks.get(queue[index]);
  if (!track) return;
  const generation = ++playGeneration;
  mediaController?.abort();
  await saveProgress();
  audio.pause(); video.pause();
  if (audio.src?.startsWith('blob:')) URL.revokeObjectURL(audio.src);
  audio.removeAttribute('src'); audio.load(); video.removeAttribute('src'); video.load();
  playQueue = [...queue]; queueIndex = index; currentVideo = undefined; currentTrack = track;
  showPlayer = true;
  renderPlayer();
  const controller = new AbortController();
  mediaController = controller;
  const timer = setTimeout(() => controller.abort(), 180000);
  let blob;
  try {
    if (available(track)) blob = await read('audio', key(track));
    if (!blob) {
      say(`Loading ${track.title}…`);
      blob = await (await api(`/api/media/${encodeURIComponent(track.id)}`, {signal: controller.signal})).blob();
      if (!blob.size || blob.size !== track.bytes) throw new Error(`Incomplete transfer: ${track.title}. Tap to retry.`);
    }
    if (generation !== playGeneration) return;
    const progress = await read('progress', progressKey(track));
    if (generation !== playGeneration) return;
    if (audio.src?.startsWith('blob:')) URL.revokeObjectURL(audio.src);
    audio.src = URL.createObjectURL(blob);
    audio.onloadedmetadata = () => {
      if (progress?.position > 0 && progress.position < audio.duration - 1) audio.currentTime = progress.position;
    };
    await saveQueue();
    try { await audio.play(); say(`Playing ${track.title}.`); }
    catch (_) { say('Ready to play. Tap the audio play control.'); }
    if (!available(track)) {
      try {
        await write('audio', key(track), blob);
        audioKeys.add(key(track));
        renderSummary();
      } catch (_) { say('Playing online. Phone storage is full or unavailable; this song was not saved.'); }
    }
    renderPlayer();
  } catch (error) {
    if (generation === playGeneration) throw error;
  } finally { clearTimeout(timer); }
}
async function playVideo(index, queue = videoQueue) {
  if (!manifest || index < 0 || index >= queue.length) return;
  const item = catalog.videos.get(queue[index]);
  if (!item) return;
  const generation = ++playGeneration;
  mediaController?.abort();
  await saveProgress(); audio.pause(); video.pause();
  const response = await api('/api/video-session', {method: 'POST', body: JSON.stringify({id: item.id})});
  if (generation !== playGeneration) return;
  const session = await response.json();
  currentTrack = undefined; currentVideo = item; videoQueue = [...queue]; videoIndex = index;
  video.src = session.url;
  showPlayer = true;
  renderPlayer();
  try { await video.play(); say(`Playing ${item.title}.`); }
  catch (_) { say('Video ready. Tap its play control. Your phone must support the original video codec.'); }
}
async function adjacent(delta) {
  if (currentVideo) {
    let index = videoIndex + delta;
    if (repeat) index = (index + videoQueue.length) % videoQueue.length;
    return playVideo(index);
  }
  let index = queueIndex + delta;
  if (shuffle && playQueue.length > 1 && delta > 0) index = (queueIndex + 1 + Math.floor(Math.random() * (playQueue.length - 1))) % playQueue.length;
  else if (repeat) index = (index + playQueue.length) % playQueue.length;
  return play(index);
}
async function togglePlayback() {
  const media = currentMedia();
  if (!media.paused) media.pause();
  else if (!media.getAttribute('src')) {
    if (currentVideo) await playVideo(videoIndex);
    else await play(queueIndex);
  } else await media.play();
  renderPlayer();
}
async function saveProgress() {
  if (!currentTrack || !device || !Number.isFinite(audio.currentTime) || !audio.getAttribute('src')) return;
  const track = currentTrack, id = progressKey(track), completed = audio.ended;
  const position = completed ? 0 : audio.currentTime, authToken = device.token;
  await transaction(['settings', 'progress', 'pending'], tx => {
    const settings = tx.objectStore('settings'), request = settings.get(`sequence:${authToken}`);
    request.onsuccess = () => {
      const seq = (request.result || 0) + 1;
      settings.put(seq, `sequence:${authToken}`);
      tx.objectStore('progress').put({position, completed}, id);
      tx.objectStore('pending').put({key: id, token: authToken, seq, id: track.id, position, completed}, id);
    };
  });
  await updateSummary();
}
async function sync() {
  if (syncing || !device) return;
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
        const updates = batch.map(({seq, id, position, completed}) => ({seq, id, position, completed}));
        const response = await api('/api/progress', {
          method: 'POST', body: JSON.stringify({updates}),
        }, {token});
        const ack = await response.json();
        await transaction('pending', tx => {
          const store = tx.objectStore('pending');
          for (const sent of batch) {
            const request = store.get(sent.key);
            request.onsuccess = () => {
              if (request.result?.seq === sent.seq && request.result?.token === token && ack.ack.includes(sent.seq)) store.delete(sent.key);
            };
          }
        });
      }
    }
    say('Listening progress returned to your desktop.');
  } finally { syncing = false; $('sync').disabled = false; await updateSummary(); }
}
async function forget() {
  if (!confirm('Remove all saved audio from this browser? Listening progress and pairing will be kept.')) return;
  await saveProgress();
  stopPlayback();
  await transaction('audio', tx => tx.objectStore('audio').clear());
  audioKeys.clear();
  render(); say('Saved audio removed.');
}
async function unpair() {
  if (!confirm('Return pending progress, revoke this pairing in all browser contexts, and remove saved audio here? Your desktop must be reachable.')) return;
  await saveProgress();
  try {
    await sync();
  } catch (error) {
    if (error.status !== 403) throw error;
    if (!confirm('Full sharing is off, so progress could not return. Unpair anyway and discard this pairing’s unsent updates?')) return;
  }
  await api('/api/forget', {method: 'POST'});
  const pending = await all('pending');
  const token = device.token;
  stopPlayback();
  await transaction(['settings', 'audio', 'art', 'pending'], tx => {
    for (const name of ['device', 'manifest', 'playback', 'navigation']) tx.objectStore('settings').delete(name);
    tx.objectStore('audio').clear(); tx.objectStore('art').clear();
    for (const update of pending) {
      if (update.token === token) tx.objectStore('pending').delete(update.key);
    }
  });
  audioKeys.clear(); device = undefined; acceptManifest(undefined);
  render(); say('This pairing has been revoked.');
}
async function reconnect() {
  if (!db || reconnecting || pendingPair || changingPairing) return;
  reconnecting = true;
  try {
    // A newly installed iOS app may inherit the narrowly scoped HttpOnly
    // cookie. Resuming recovers the original grant; it never creates one.
    const resumed = await (await api('/api/resume', {method: 'POST'}, device || null)).json();
    if (changingPairing || pendingPair) return;
    if (!device) {
      device = resumed;
      await write('settings', 'device', device);
    }
    needsPairing = false;
    const sequenceKey = `sequence:${device.token}`;
    const sequence = Math.max((await read('settings', sequenceKey)) || 0, resumed.last_seq || 0);
    await write('settings', sequenceKey, sequence);
    await refresh();
    await sync();
    say('Your music is ready.');
  } catch (error) {
    if (device && error.status === 401) {
      needsPairing = true;
      say('This desktop pairing was revoked. Saved songs still play here; scan a new QR to reconnect.');
    } else if (error.status === 403) {
      say(error.message);
    } else if (device) {
      say('Working offline. Saved songs and progress remain on this phone.');
    } else {
      say('Scan the QR in desktop Devices to pair this phone.');
    }
  } finally {
    reconnecting = false;
    render();
  }
}
function bind(id, action) { $(id).onclick = () => Promise.resolve().then(action).catch(fail); }
for (const media of [audio, video]) {
  media.addEventListener('play', renderPlayer);
  media.addEventListener('pause', () => { saveProgress().catch(fail); renderPlayer(); });
  media.addEventListener('ended', () => saveProgress().then(() => adjacent(1)).catch(fail));
}
audio.addEventListener('timeupdate', () => {
  if (Date.now() - lastSaved > 5000) { lastSaved = Date.now(); saveProgress().catch(fail); }
});
audio.addEventListener('seeked', () => saveProgress().catch(fail));
audio.addEventListener('error', () => say('This audio format could not play. Try an MP3, AAC, or FLAC original supported by your phone.'));
video.addEventListener('error', () => say('This video could not play. Check desktop sharing and codec support; H.264/AAC MP4 is widely supported.'));
document.addEventListener('visibilitychange', () => {
  if (document.hidden) saveProgress().catch(fail);
  else reconnect().catch(fail);
});
window.addEventListener('online', () => reconnect().catch(fail));
for (const [id, action] of [
  ['pair', pair], ['sync', sync], ['refresh', refresh], ['forget', forget], ['unpair', unpair],
  ['previous', () => adjacent(-1)], ['next', () => adjacent(1)], ['mini-next', () => adjacent(1)],
  ['mini-toggle', togglePlayback], ['back', back], ['player-back', back],
]) bind(id, action);
bind('change-pairing', () => {
  changingPairing = true;
  rememberPair('');
  $('pair-code').value = '';
  showPlayer = false;
  $('settings').open = false;
  render();
  $('pairing').scrollIntoView?.({block: 'start'});
  $('pair-code').focus?.();
  say('Enter a new pairing link, or keep your current pairing.');
});
bind('cancel-pair', () => {
  changingPairing = false;
  rememberPair('');
  $('pair-code').value = '';
  render();
  say('Kept your current pairing.');
});
bind('mini-open', () => { showPlayer = true; renderPlayer(); window.scrollTo?.({top: 0}); });
bind('show-queue', () => navigate({detail: {type: currentVideo ? 'video-queue' : 'queue'}, search: ''}));
bind('now-album', () => { if (currentTrack) navigate({pivot: 'albums', detail: {type: 'album', id: Catalog.albumId(currentTrack)}}); });
bind('offline-filter', () => { view.offline = !view.offline; pageSize = 80; saveNavigation(); render(); });
bind('more', () => { pageSize += 80; renderBrowse(); });
bind('play-view', () => {
  const result = visible(), ids = result.items.map(item => item.id);
  return result.kind === 'videos' ? playVideo(0, ids) : play(0, ids);
});
bind('shuffle-view', () => {
  const ids = visible().items.map(track => track.id);
  for (let i = ids.length - 1; i > 0; i--) { const j = Math.floor(Math.random() * (i + 1)); [ids[i], ids[j]] = [ids[j], ids[i]]; }
  return play(0, ids);
});
bind('queue-view', () => addQueue(visible().items.map(track => track.id)));
bind('shuffle', () => { shuffle = !shuffle; renderPlayer(); });
bind('repeat', () => { repeat = !repeat; renderPlayer(); });
$('search').oninput = () => { view.search = $('search').value; pageSize = 80; renderBrowse(); saveNavigation(); };
for (const button of document.querySelectorAll('[data-pivot]')) button.onclick = () => navigate({pivot: button.dataset.pivot, detail: null});
async function prepareOffline() {
  if (!isSecureContext || !('serviceWorker' in navigator)) {
    $('offline-status').textContent = 'Offline reopening needs trusted HTTPS and service-worker support. Online pairing and playback still work.';
    return;
  }
  try {
    const registration = await navigator.serviceWorker.register('/sw.js');
    const updateNotice = () => {
      if (registration.waiting) $('offline-status').textContent = 'Player update ready. Close all Gnune tabs and reopen. Pairing and saved songs are kept.';
    };
    registration.addEventListener('updatefound', () => registration.installing?.addEventListener('statechange', updateNotice));
    await navigator.serviceWorker.ready;
    $('offline-status').textContent = 'Offline player ready. Songs you play are saved when storage allows; videos stream online only.';
    updateNotice();
  } catch (_) { $('offline-status').textContent = 'Offline player could not be prepared. Check certificate trust and reopen while connected.'; }
}
(async () => {
  const scanned = new URLSearchParams(location.hash.slice(1)).get('pair');
  let saved = '', invalidLink = false;
  try { saved = sessionStorage.getItem('gnune-pair') || ''; } catch (_) { /* Tab storage can be restricted. */ }
  if (scanned || saved) {
    try { rememberPair(pairingCode(scanned || saved)); $('pair-code').value = pendingPair; }
    catch (_) { rememberPair(''); invalidLink = true; }
    history.replaceState(null, '', '/');
  }
  db = await openDB();
  const stored = await Promise.all([
    read('settings', 'device'), read('settings', 'manifest'),
    all('audio', true), all('art', true),
    read('settings', 'navigation'), read('settings', 'playback'),
  ]);
  device = stored[0]; acceptManifest(stored[1]); audioKeys = new Set(stored[2]); artKeys = stored[3];
  view.pivot = manifest?.mode === 'library' ? 'artists' : 'songs';
  if (manifest && stored[4]?.scope === manifest.scope) { view = stored[4].view; backStack = stored[4].backStack || []; }
  if (manifest && stored[5]?.scope === manifest.scope) {
    playQueue = stored[5].tracks.filter(id => catalog.tracks.has(id));
    queueIndex = Math.min(stored[5].index, playQueue.length - 1);
    currentTrack = catalog.tracks.get(playQueue[queueIndex]);
  }
  render(); $('pair').disabled = false; prepareOffline().catch(fail); await updateSummary();
  if (invalidLink) say('Invalid pairing link. Scan a fresh QR from desktop Devices.');
  else if (pendingPair) say('Pairing link ready.');
  else await reconnect();
})().catch(fail);
