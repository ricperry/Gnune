'use strict';
const $ = id => document.getElementById(id);
const Catalog = globalThis.GnuneCatalog;
let db, device, manifest, catalog = Catalog.build(null), pendingPair = '', needsPairing = false;
let changingPairing = false;
let audioKeys = new Set(), artKeys = [], currentTrack, currentVideo, currentPodcast;
let sharedState, sharedRefreshing = false, podcastQueue = [], podcastIndex = -1, progressStarted = false;
const podcastShows = new Map();
let activePodcastShow, activePodcastEpisodes = [], podcastOpenGeneration = 0;
let playQueue = [], queueIndex = -1, videoQueue = [], videoIndex = -1;
let lastSaved = 0, syncing = false, reconnecting = false, pendingCount = 0, showPlayer = false;
let playGeneration = 0, mediaController, shuffle = false, repeat = false;
let view = {area: 'quickplay', pivot: 'artists', detail: null, offline: false, search: ''}, backStack = [], pageSize = 80;
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
  if (!next) { sharedState = undefined; podcastShows.clear(); activePodcastShow = undefined; activePodcastEpisodes = []; podcastQueue = []; podcastIndex = -1; }
  if (manifest && next && manifest.scope !== next.scope) { sharedState = undefined; podcastShows.clear(); }
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
  await refreshShared();
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
  const track = currentVideo || currentPodcast || currentTrack;
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
  const count = currentVideo ? videoQueue.length : currentPodcast ? podcastQueue.length : playQueue.length;
  const index = currentVideo ? videoIndex : currentPodcast ? podcastIndex : queueIndex;
  const transferNote = currentVideo
    ? 'video streams while playing; no offline video download'
    : currentPodcast ? 'podcast streams while playing; no automatic download'
    : 'only the playing song transfers; played songs are saved when storage allows';
  $('queue-summary').textContent = `${index + 1} of ${count} · ${transferNote}`;
  $('shuffle').hidden = Boolean(currentVideo || currentPodcast);
  $('repeat').setAttribute('aria-pressed', String(repeat));
  $('shuffle').setAttribute('aria-pressed', String(shuffle));
}
function render() {
  $('pairing').hidden = Boolean(device) && !pendingPair && !needsPairing && !changingPairing;
  $('replace-help').hidden = !device || (!pendingPair && !changingPairing);
  $('cancel-pair').hidden = !device || (!pendingPair && !changingPairing);
  $('manual-pairing').open = !pendingPair;
  $('pair-help').textContent = changingPairing
    ? 'Choose Pair phone in desktop Devices, then paste its fresh Tailscale link below. Your current pairing stays active until this succeeds.'
    : pendingPair ? 'Your link is ready. Tap Pair phone.' : 'Connect Tailscale, then scan the QR in desktop Devices or enter its link below.';
  $('scope-label').textContent = manifest?.mode === 'library' ? 'your desktop collection' : 'shared desktop queue';
  $('connection').textContent = device ? 'paired to your desktop' : 'on your phone';
  $('settings').hidden = !device;
  for (const button of document.querySelectorAll('[data-pivot]')) {
    button.setAttribute('aria-current', button.dataset.pivot === view.pivot ? 'page' : 'false');
    button.hidden = false;
  }
  $('area-title').textContent = view.area || 'music';
  $('library').dataset.area = view.area || 'music';
  $('library').dataset.detail = view.detail ? 'true' : 'false';
  $('pivots').hidden = (view.area || 'music') !== 'music';
  $('video-pivots').hidden = view.area !== 'videos';
  for (const button of document.querySelectorAll('[data-area]')) button.setAttribute('aria-current', button.dataset.area === view.area ? 'page' : 'false');
  $('offline-filter').hidden = ['podcasts', 'videos', 'quickplay'].includes(view.area);
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
  if (view.area === 'quickplay') { renderQuickplay(); return; }
  if (view.area === 'podcasts') { renderPodcasts(); return; }
  if (view.area === 'videos' && !manifest.includes_video) { disabledArea('videos'); return; }
  const result = visible();
  $('view-title').textContent = result.title;
  $('view-subtitle').textContent = result.subtitle;
  $('album-pin').hidden = result.kind !== 'tracks' || view.detail?.type !== 'album' || manifest.mode !== 'library' || !sharedState;
  if (!$('album-pin').hidden) { const pinned=(sharedState.pins||[]).some(p=>p.id===view.detail.id); $('album-pin').textContent=pinned?'unpin album':'pin album'; $('album-pin').onclick=()=>changePin(view.detail.id,!pinned).catch(fail); }
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
  if (!result.items.length) target.append(node('p', 'empty', view.offline ? 'No saved songs in this view. Play a song while connected to save it here.' : 'No items in this view.'));
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
      tx.objectStore('settings').delete('podcastPlayback');
    });
    stopPlayback();
    device = nextDevice; needsPairing = false;
    changingPairing = false;
    acceptManifest(undefined);
    rememberPair(''); $('pair-code').value = '';
    view = {area: 'quickplay', pivot: nextDevice.mode === 'library' ? 'artists' : 'songs', detail: null, offline: false, search: ''};
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
  currentTrack = undefined; currentVideo = undefined; currentPodcast = undefined;
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
  playQueue = [...queue]; queueIndex = index; currentVideo = undefined; currentPodcast = undefined; currentTrack = track; track.conflicted = false; progressStarted = true;
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
    const progress = await resumeProgress(track);
    if (generation !== playGeneration) return;
    if (audio.src?.startsWith('blob:')) URL.revokeObjectURL(audio.src);
    audio.src = URL.createObjectURL(blob);
    audio.onloadedmetadata = () => {
      if (progress?.position > 0 && progress.position < audio.duration - 1) audio.currentTime = progress.position;
    };
    await saveQueue();
    try { await audio.play(); say(`Playing ${track.title}.`); }
    catch (_) { say('Ready to play. Tap the audio play control.'); }
    saveProgress().then(() => sync(false)).catch(fail);
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
  const progress = await resumeProgress(item);
  if (generation !== playGeneration) return;
  currentTrack = undefined; currentPodcast = undefined; currentVideo = item; item.conflicted = false; progressStarted = true; videoQueue = [...queue]; videoIndex = index;
  video.src = session.url;
  video.onloadedmetadata = () => restorePosition(video, progress);
  showPlayer = true;
  renderPlayer();
  try { await video.play(); say(`Playing ${item.title}.`); saveProgress().then(() => sync(false)).catch(fail); }
  catch (_) { say('Video ready. Tap its play control. Your phone must support the original video codec.'); }
}
async function adjacent(delta, skipCompleted = false) {
  if (currentPodcast) {
    let index = podcastIndex + delta;
    while (index >= 0 && index < podcastQueue.length) {
      const item = activePodcastEpisodes.find(ep=>ep.id===podcastQueue[index]);
      const local = item && await read('progress',progressKey(item));
      if (!skipCompleted || !(local || item?.progress)?.completed) {await playPodcast(index);return;}
      index += delta;
    }
    say('No more unplayed episodes.'); return;
  }
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
    else if (currentPodcast) await playPodcast(podcastIndex);
    else await play(queueIndex);
  } else await media.play();
  renderPlayer();
}
async function saveProgress() {
  const track = currentVideo || currentPodcast || currentTrack, media = currentMedia();
  if (!track || track.conflicted || !device || !Number.isFinite(media.currentTime) || !media.getAttribute('src')) return;
  const id = progressKey(track), completed = media.ended;
  const position = completed ? 0 : media.currentTime, authToken = device.token;
  const remote = track.progress || sharedState?.progress?.[track.id];
  const started = progressStarted; progressStarted = false;
  await transaction(['settings', 'progress', 'pending'], tx => {
    const settings = tx.objectStore('settings'), request = settings.get(`sequence:${authToken}`);
    request.onsuccess = () => {
      const seq = (request.result || 0) + 1;
      settings.put(seq, `sequence:${authToken}`);
      const pending = tx.objectStore('pending'), previous = pending.get(id);
      previous.onsuccess = () => {
        const old = previous.result?.token === authToken ? previous.result : undefined;
        const base = old?.base || remote?.revision;
        tx.objectStore('progress').put({position, completed, revision: base}, id);
        pending.put({key: id, token: authToken, seq, id: track.id, position, completed, base, history: Boolean(old?.history || started), last_base: old?.last_base || sharedState?.last_revision}, id);
      };
    };
  });
  await updateSummary();
}
async function sync(capture = true) {
  if (syncing || !device) return;
  syncing = true; $('sync').disabled = true;
  let conflict = Boolean((currentVideo || currentPodcast || currentTrack)?.conflicted);
  try {
    if (capture) await saveProgress();
    const updates = await all('pending'), groups = new Map();
    for (const update of updates) {
      if (!groups.has(update.token)) groups.set(update.token, []);
      groups.get(update.token).push(update);
    }
    for (const [token, records] of groups) {
      records.sort((a, b) => a.seq - b.seq);
      for (let start = 0; start < records.length; start += 100) {
        const batch = records.slice(start, start + 100);
        const modern = batch.filter(item => item.base);
        for (const sent of modern) {
          try {
            const saved = await (await api('/api/state', {method: 'POST', body: JSON.stringify({action:'progress', id:sent.id, position:sent.position, completed:sent.completed, base:sent.base, history:sent.history, last_base:sent.last_base})}, {token})).json();
            sent.accepted = saved.progress;
            if (device?.token === token && sharedState && saved.progress) { sharedState.progress ||= {}; sharedState.progress[sent.id] = saved.progress; const current = currentVideo || currentPodcast || currentTrack; if (current?.id === sent.id) current.progress = saved.progress; }
          } catch (error) {
            if (error.status !== 409) throw error;
            conflict = true;
            const current = currentVideo || currentPodcast || currentTrack; if (current?.id === sent.id) current.conflicted = true;
            say('A newer listening position was kept. Reopen the item to resume from it.');
          }
          await transaction(['pending', 'progress'], tx => {
            const store = tx.objectStore('pending'), request = store.get(sent.key);
            request.onsuccess = () => {
              const pending = request.result;
              if (pending?.token !== token) return;
              if (pending.seq === sent.seq) store.delete(sent.key);
              else if (pending.base === sent.base && sent.accepted) {
                pending.base = sent.accepted.revision;
                store.put(pending, sent.key);
              }
              if (sent.accepted) {
                const progress = tx.objectStore('progress'), local = progress.get(sent.key);
                local.onsuccess = () => {
                  if (local.result) progress.put({...local.result, revision: sent.accepted.revision}, sent.key);
                };
              }
            };
          });
        }
        const legacy = batch.filter(item => !item.base);
        if (!legacy.length) continue;
        const updates = legacy.map(({seq, id, position, completed}) => ({seq, id, position, completed}));
        const response = await api('/api/progress', {
          method: 'POST', body: JSON.stringify({updates}),
        }, {token});
        const ack = await response.json();
        await transaction('pending', tx => {
          const store = tx.objectStore('pending');
          for (const sent of legacy) {
            const request = store.get(sent.key);
            request.onsuccess = () => {
              if (request.result?.seq === sent.seq && request.result?.token === token && ack.ack.includes(sent.seq)) store.delete(sent.key);
            };
          }
        });
      }
    }
    await refreshShared();
    if (conflict) say('A newer listening position was kept. Reopen the item to resume from it.');
    else if (updates.length) say('Listening progress returned to your desktop.');
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
  media.addEventListener('play', () => {
    if (media === currentMedia()) progressStarted = true;
    renderPlayer();
  });
  media.addEventListener('pause', () => { saveProgress().catch(fail); renderPlayer(); });
  media.addEventListener('ended', () => saveProgress().then(() => sync(false)).then(() => adjacent(1,true)).catch(fail));
}
for (const media of [audio, video]) media.addEventListener('timeupdate', () => {
  if (Date.now() - lastSaved > 5000) { lastSaved = Date.now(); saveProgress().catch(fail); }
});
for (const media of [audio, video]) media.addEventListener('seeked', () => saveProgress().then(() => sync(false)).catch(fail));
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
  ['rewind', rewind], ['mini-toggle', togglePlayback], ['back', back], ['player-back', back],
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
bind('now-album', () => { if (currentPodcast) navigate({area:'podcasts',detail:{type:'podcast',id:currentPodcast.show}}); else if (currentTrack) navigate({area:'music',pivot: 'albums', detail: {type: 'album', id: Catalog.albumId(currentTrack)}}); });
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
for (const button of document.querySelectorAll('[data-pivot]')) button.onclick = () => navigate({area: button.dataset.pivot.startsWith('video') ? 'videos' : 'music', pivot: button.dataset.pivot, detail: null});
for (const button of document.querySelectorAll('[data-area]')) button.onclick = () => navigate({area: button.dataset.area, pivot: button.dataset.area === 'videos' ? 'videos' : 'artists', detail:null});
async function refreshShared() {
  if (!device || sharedRefreshing) return;
  const credential = device;
  sharedRefreshing = true;
  try {
    const next = await (await api('/api/quickplay', {
    }, credential)).json();
    if (device?.token !== credential.token || changingPairing) return;
    const changed = JSON.stringify(sharedState) !== JSON.stringify(next);
    sharedState = next;
    await write('settings', `shared:${credential.token}`, next);
    if (changed && (view.area === 'quickplay' || (view.area === 'podcasts' && !view.detail))) renderBrowse();
    const opened = view.detail?.type === 'podcast' ? podcastShows.get(view.detail.id) : undefined;
    const order = next.podcasts?.find(show=>show.id===opened?.id)?.order;
    if (opened && order && order !== opened.order) {
      const updated=await(await api(`/api/podcast-show/${encodeURIComponent(opened.id)}`)).json();
      podcastShows.set(opened.id,updated);
      reorderPodcastQueue(updated);
      renderBrowse();
    }
  }
  catch (error) {
    if (error.status !== 404 && !sharedState) throw error;
  }
  finally {
    sharedRefreshing = false;
  }
}
async function resumeProgress(item) {
  const pending = await read('pending', progressKey(item));
  if (pending?.token === device.token) return pending;
  if (sharedState) {
    try {
      const progress = await (await api(`/api/media-state/${encodeURIComponent(item.id)}`)).json();
      sharedState.progress ||= {
      };
      sharedState.progress[item.id] = progress;
      item.progress = progress;
      return progress;
    }
    catch (error) {
      if (error.status && error.status !== 404) throw error;
    }
  }
  return item.progress || sharedState?.progress?.[item.id] || await read('progress', progressKey(item));
}
function restorePosition(media, progress) {
  if (progress?.position > 0 && !progress.completed && (!Number.isFinite(media.duration) || progress.position < media.duration - 1)) media.currentTime = progress.position;
}
function disabledArea(area) {
  $('view-actions').hidden = true;
  $('album-pin').hidden=true;
  $('more').hidden = true;
  $('view-title').textContent = area;
  $('view-subtitle').textContent = '';
  $('browse').replaceChildren(node('p', 'empty', `To access ${area}, choose Pair phone in desktop Devices and pair again with its new Tailscale QR. Older pairings keep their original permissions.`));
}
function quickAlbum(section, entry, resume) {
  const album = catalog.albums.get(entry.id);
  if (!album) return;
  const row = node('div', 'quick-album'), button = node('button', 'album-tile');
  button.append(cover(album.art),node('strong','',album.title),node('small','',album.artist));
  button.onclick = () => {
    const ids=album.tracks.map(t=>t.id);
    play(resume ? Math.max(0, ids.indexOf(entry.track)) : 0,ids).catch(fail);
  };
  row.append(button);
  if (manifest.mode === 'library') {
    const pinned = (sharedState?.pins || []).some(p=>p.id === entry.id);
    const pin=node('button','row-action',pinned ? 'unpin' : 'pin');
    pin.onclick=()=>changePin(entry.id,!pinned).catch(fail);
    row.append(pin);
  }
  section.append(row);
}
function renderQuickplay() {
  $('view-actions').hidden=true;
  $('album-pin').hidden=true;
  $('more').hidden=true;
  $('view-title').textContent='quickplay';
  $('view-subtitle').textContent=manifest.mode==='library'?'shared with your desktop':'limited to this shared queue';
  const target=$('browse');
  target.replaceChildren();
  if (!sharedState) {
    target.append(node('p','empty','Reconnect to your desktop to load shared Quickplay. Your saved music remains in Music.'));
    return;
  }
  for (const [key,title] of [['pins','pins'],['new','new'],['history','history']]) {
    const section = node('section', 'quick-section');
    section.append(node('h2', '', title));
    const tiles = node('div', 'album-grid');
    for (const entry of sharedState[key] || []) quickAlbum(tiles, entry, key === 'history');
    if (!(sharedState[key] || []).length) section.append(node('p', 'muted', `No shared ${title} yet.`));
    section.append(tiles);
    target.append(section);
  }
  const last = sharedState.last_podcast;
  const show = sharedState.podcasts?.find(show => show.id === last?.show);
  if (sharedState.includes_podcasts && show) {
    const section = node('section', 'quick-section');
    section.append(node('h2', '', 'latest podcast'));
    const button = node('button', 'resume-show');
    const copy = node('span', 'item-text');
    copy.append(node('strong', '', show.title), node('small', '', 'Resume listening'));
    button.append(cover(show.art), copy);
    button.onclick = () => openPodcast(show.id, last.episode).catch(fail);
    section.append(button);
    target.prepend(section);
  }
}
async function changePin(id,pinned) {
  const previous=sharedState.pins;
  sharedState.pins=pinned?[{
    id,track:catalog.albums.get(id)?.tracks[0]?.id
  },...previous.filter(p=>p.id!==id)]:previous.filter(p=>p.id!==id);
  renderBrowse();
  try {
    await api('/api/state',{
      method:'POST',body:JSON.stringify({
        action:'pin',id,pinned
      })
    });
    await refreshShared();
  }
  catch(error){
    sharedState.pins=previous;
    renderBrowse();
    throw error;
  }
}
async function openPodcast(id, playId) {
  const opened = ++podcastOpenGeneration, playback = playGeneration;
  await refreshShared();
  if (opened !== podcastOpenGeneration || playback !== playGeneration) return;
  if (!sharedState?.includes_podcasts) {
    navigate({
      area:'podcasts',detail:null
    });
    return;
  }
  const response=await (await api(`/api/podcast-show/${encodeURIComponent(id)}`)).json();
  if (opened !== podcastOpenGeneration || playback !== playGeneration) return;
  podcastShows.set(id,response);
  navigate({
    area:'podcasts',detail:{
      type:'podcast',id
    }
  });
  reorderPodcastQueue(response);
  if(playId){
    const episodes=response.episodes;
    let index=episodes.findIndex(e=>e.id===playId);
    if(playId==='latest') index=episodes.reduce((best,ep,i)=>best<0||((ep.published||0)>(episodes[best].published||0))?i:best,-1);
    else {
      if(index<0)index=0;
      const first=index;
      while(index<episodes.length&&episodes[index].progress?.completed)index++;
      if(index===episodes.length)index=episodes.findIndex((ep,i)=>i<first&&!ep.progress?.completed);
    }
    if(index>=0&&index<episodes.length)await playPodcast(index,episodes.map(e=>e.id));
    else say('No more unplayed episodes.');
  }
}
function renderPodcasts() {
  if(!sharedState?.includes_podcasts){
    disabledArea('podcasts');
    return;
  }
  const target=$('browse');
  target.replaceChildren();
  $('more').hidden=true;
  $('view-actions').hidden=true;
  $('album-pin').hidden=true;
  $('view-title').textContent='podcasts';
  $('view-subtitle').textContent='streams on demand · listening state shared with your desktop';
  $('back').hidden=!view.detail;
  if(view.detail?.type==='podcast'){
    const show=sharedState.podcasts.find(s=>s.id===view.detail.id), detail=podcastShows.get(view.detail.id);
    $('view-title').textContent=show?.title||'podcast';
    if(!detail){
      const b=node('button','item','load episodes');
      b.onclick=()=>openPodcast(view.detail.id).catch(fail);
      target.append(b);
      return;
    }
    if (show?.summary) {
      if (show.summary.length <= 280) target.append(node('p', 'show-summary', show.summary));
      else {
        const synopsis = node('details', 'show-description');
        synopsis.append(node('summary', '', 'Show synopsis'), node('p', 'show-summary', show.summary));
        target.append(synopsis);
      }
    }
    const order=node('button','secondary',detail.order==='oldest_first'?'oldest first':'newest first');
    order.onclick=async()=>{
      try{
        await api('/api/state',{
          method:'POST',body:JSON.stringify({
            action:'order',id:detail.id,order:detail.order==='oldest_first'?'newest_first':'oldest_first'
          })
        });
        await openPodcast(detail.id);
      }
      catch(e){
        fail(e);
      }
    };
    target.append(order);
    const episodes=detail.episodes.filter(e=>!view.search||e.title.toLocaleLowerCase().includes(view.search.toLocaleLowerCase()));
    for(const ep of episodes.slice(0,pageSize)){
      const b = node('button', 'episode-row item');
      b.append(node('strong', '', ep.title), node('small', 'episode-summary', ep.summary || ''));
      b.onclick=()=>playPodcast(detail.episodes.findIndex(e=>e.id===ep.id),detail.episodes.map(e=>e.id)).catch(fail);
      target.append(b);
    }
    $('more').hidden=episodes.length<=pageSize;
    return;
  }
  const tiles = node('div', 'album-grid');
  for (const show of sharedState.podcasts || []) {
    if (view.search && !`${show.title} ${show.publisher}`.toLocaleLowerCase().includes(view.search.toLocaleLowerCase())) continue;
    const button = node('button', 'album-tile');
    button.append(cover(show.art), node('strong', '', show.title), node('small', '', show.publisher));
    button.onclick = () => openPodcast(show.id).catch(fail);
    tiles.append(button);
  }
  target.append(tiles);
  if(!sharedState.podcasts?.length)target.append(node('p','empty','Subscribe to podcasts on your desktop. Their shows will appear here.'));
}
function reorderPodcastQueue(detail) {
  if(activePodcastShow !== detail.id) return;
  activePodcastEpisodes=detail.episodes;
  podcastQueue=detail.episodes.map(ep=>ep.id);
  podcastIndex=podcastQueue.indexOf(currentPodcast?.id);
  savePodcastQueue().catch(fail);
}
function savePodcastQueue() {
  return write('settings','podcastPlayback',{
    scope:manifest.scope,token:device.token,show:activePodcastShow,episodes:activePodcastEpisodes,index:podcastIndex
  });
}
async function playPodcast(index, queue) {
  let detail;
  if(queue){
    detail=[...podcastShows.values()].find(s=>s.episodes.some(ep=>ep.id===queue[index]));
  }
  else detail={
    id:activePodcastShow,episodes:activePodcastEpisodes
  };
  const ids=queue||podcastQueue;
  const item=detail?.episodes.find(ep=>ep.id===ids[index]);
  if(!item)return;
  const generation=++playGeneration;
  mediaController?.abort();
  await saveProgress();
  if(generation!==playGeneration)return;
  audio.pause();
  video.pause();
  // Refresh authorization and the trusted episode mapping after desktop restart.
  const fresh=await(await api(`/api/podcast-show/${encodeURIComponent(detail.id)}`)).json();
  if(generation!==playGeneration)return;
  podcastShows.set(detail.id,fresh);
  const selected=fresh.episodes.find(ep=>ep.id===item.id);
  if(!selected)throw new Error('This episode is no longer listed in the shared feed.');
  const session=await(await api('/api/podcast-session',{
    method:'POST',body:JSON.stringify({
      action:'play',id:item.id
    })
  })).json();
  if(generation!==playGeneration)return;
  const progress=await resumeProgress(selected);
  if(generation!==playGeneration)return;
  if(audio.src?.startsWith('blob:'))URL.revokeObjectURL(audio.src);
  currentTrack=undefined;
  currentVideo=undefined;
  currentPodcast=selected;
  selected.conflicted=false;
  activePodcastShow=fresh.id;
  activePodcastEpisodes=fresh.episodes;
  podcastQueue=fresh.episodes.map(ep=>ep.id);
  podcastIndex=podcastQueue.indexOf(selected.id);
  progressStarted=true;
  audio.src=session.url;
  video.removeAttribute('src');
  video.load();
  audio.onloadedmetadata=()=>restorePosition(audio,progress);
  showPlayer=true;
  renderPlayer();
  await savePodcastQueue();
  if(generation!==playGeneration)return;
  try{
    await audio.play();
    if(generation===playGeneration)saveProgress().then(()=>sync(false)).catch(fail);
  }
  catch(_){
    if(generation===playGeneration)say('Episode ready. Tap play to listen.');
  }
}
async function rewind(){
  const media=currentMedia();
  if(!Number.isFinite(media.currentTime)||!media.getAttribute('src'))return;
  const paused=media.paused;
  media.currentTime=Math.max(0,media.currentTime-10);
  if(paused)media.pause();
  await saveProgress();
  await sync();
}
if (typeof setInterval === 'function') setInterval(()=>{
  if (device && !document.hidden && !changingPairing) sync().catch(() => {});
},15000);
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
  if (!view.area) view.area = view.pivot.startsWith('video') ? 'videos' : 'music';
  sharedState = await read('settings', `shared:${device?.token}`);
  const podcastPlayback = await read('settings','podcastPlayback');
  if(podcastPlayback && podcastPlayback.scope===manifest?.scope && podcastPlayback.token===device?.token && podcastPlayback.episodes?.length){activePodcastShow=podcastPlayback.show;activePodcastEpisodes=podcastPlayback.episodes;podcastQueue=activePodcastEpisodes.map(e=>e.id);podcastIndex=podcastPlayback.index;}
  render(); $('pair').disabled = false; prepareOffline().catch(fail); await updateSummary();
  if (invalidLink) say('Invalid pairing link. Scan a fresh QR from desktop Devices.');
  else if (pendingPair) say('Pairing link ready.');
  else await reconnect();
})().catch(fail);
