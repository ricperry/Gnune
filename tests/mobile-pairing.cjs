// Run with: node tests/mobile-pairing.cjs
const assert = require('node:assert/strict');
const fs = require('node:fs');
const test = require('node:test');
const vm = require('node:vm');
const source = fs.readFileSync('src/mobile/catalog.js', 'utf8') + '\n' + fs.readFileSync('src/mobile/app.js', 'utf8');
const code = 'a'.repeat(48);
const queue = {scope: 'album', tracks: [{id: 'song', version: '1', title: 'Song', artist: 'Artist', album: 'Album', bytes: 8}]};
const tick = () => new Promise(resolve => setImmediate(resolve));

async function companion({scan = '', paired = false, offline = false, savedCode = '', cookie = false, revoked = false, catalogData = queue, pairingCatalog = catalogData, savedAudio = [], savedPending = [], sharedData, podcastData, rejectState = false} = {}) {
  const elements = new Map();
  const element = () => {
    const attributes = new Map();
    return {hidden: false, disabled: true, paused: true, currentTime: 0, duration: 300, ended: false, value: '', textContent: '', src: '', dataset: {},
      classList: {add() {}}, append() {}, prepend() {}, replaceChildren() {}, addEventListener() {},
      pause() { this.paused = true; }, async play() { this.onloadedmetadata?.(); this.paused = false; }, load() {},
      setAttribute(key, value) { attributes.set(key, value); },
      getAttribute(key) { return key === 'src' ? this.src : attributes.get(key); },
      removeAttribute(key) { attributes.delete(key); if (key === 'src') this.src = ''; }};
  };
  const get = id => { if (!elements.has(id)) elements.set(id, element()); return elements.get(id); };
  const stores = Object.fromEntries(['settings', 'audio', 'progress', 'pending', 'art'].map(name => [name, new Map()]));
  if (paired) { stores.settings.set('device', {token: 'old', scope: 'album'}); stores.settings.set('manifest', catalogData); }
  for (const [key, blob] of savedAudio) stores.audio.set(key, blob);
  for (const update of savedPending) stores.pending.set(update.key, update);
  let activeCatalog = catalogData;
  const sessions = new Map(savedCode ? [['gnune-pair', savedCode]] : []);
  const requests = [], reads = [];
  let connected = !offline, rejectMutations = rejectState;
  const database = {
    transaction(name) {
      const tx = {objectStore(store) {
        const map = stores[store];
        const request = result => {
          const req = {result}; queueMicrotask(() => req.onsuccess?.()); return req;
        };
        return {get: key => { reads.push({store, key}); return request(map.get(key)); }, getAll: () => request([...map.values()]), getAllKeys: () => request([...map.keys()]), clear: () => map.clear(), put: (value, key) => map.set(key, value), delete: key => map.delete(key)};
      }};
      setImmediate(() => tx.oncomplete?.()); return tx;
    },
  };
  const registration = {addEventListener() {}};
  const context = vm.createContext({
    document: {getElementById: get, createElement: element, addEventListener() {}, querySelectorAll() { return []; }},
    indexedDB: {open() { const request = {result: database}; queueMicrotask(() => request.onsuccess()); return request; }},
    sessionStorage: {getItem: key => sessions.get(key), setItem: (key, value) => sessions.set(key, value), removeItem: key => sessions.delete(key)},
    location: {origin: 'https://gnune.test', hash: scan ? `#pair=${scan}` : ''},
    history: {replaceState() {}},
    navigator: {serviceWorker: {register: async () => registration, ready: new Promise(() => {})}},
    isSecureContext: true,
    window: {addEventListener() {}},
    URL, URLSearchParams, Headers, AbortSignal, AbortController, setTimeout, clearTimeout, Blob,
    fetch: async (path, options) => {
      requests.push({path, options});
      if (!connected) throw new Error('Desktop unavailable');
      if (path === '/api/pair') {
        activeCatalog = pairingCatalog;
        return {ok: true, json: async () => ({token: 'new', scope: activeCatalog.scope, mode: activeCatalog.mode})};
      }
      if (path === '/api/resume') return (paired || cookie) && !revoked ? {ok: true, json: async () => ({token: 'old', scope: 'album', last_seq: 20})} : {ok: false, status: 401, text: async () => 'Pair this phone using the desktop share link'};
      if (path === '/api/manifest') return {ok: true, json: async () => activeCatalog};
      if (path === '/api/quickplay') return sharedData ? {ok:true,json:async()=>structuredClone(sharedData)} : {ok:false,status:404,text:async()=> 'Unsupported by old desktop'};
      if (path.startsWith('/api/media-state/')) {
        const id=decodeURIComponent(path.slice('/api/media-state/'.length));
        const ep=podcastData?.episodes.find(ep=>ep.id===id);
        const value=sharedData?.progress?.[id] || ep?.progress;
        return value ? {ok:true,json:async()=>structuredClone(value)} : {ok:false,status:404,text:async()=> 'Unknown'};
      }
      if (path.startsWith('/api/podcast-show/')) return {ok:true,json:async()=>structuredClone(podcastData)};
      if (path === '/api/podcast-session') return {ok:true,json:async()=>({url:'/api/podcast-media/episode'})};
      if (path === '/api/state') {
        const mutation=JSON.parse(options.body);
        if(rejectMutations) return {ok:false,status:409,text:async()=> 'Newer progress exists'};
        const progress={position:mutation.position||0,completed:Boolean(mutation.completed),revision:`r${mutation.position||0}`};
        if(mutation.action==='progress' && sharedData) {sharedData.progress ||= {}; sharedData.progress[mutation.id]=progress;}
        return {ok:true,json:async()=>({saved:true,progress})};
      }
      if (path === '/api/progress') return {ok:true,json:async()=>({ack:JSON.parse(options.body).updates.map(u=>u.seq)})};
      if (path.startsWith('/api/media/')) return {ok: true, blob: async () => new Blob(['12345678'])};
      if (path === '/api/video-session') return {ok: true, json: async () => ({url: '/api/video/clip'})};
      throw new Error(`Unexpected request: ${path}`);
    },
  });
  const initialized = vm.runInContext(source, context);
  for (let i = 0; i < 20 && get('pair').disabled; i++) await tick();
  await initialized;
  return {get, stores, sessions, requests, reads, context, setOnline(value) { connected = value; }, setRejectState(value) { rejectMutations = value; }};
}

test('scan exposes deliberate pairing before offline worker becomes ready, without downloading', async () => {
  const app = await companion({scan: code});
  assert.equal(app.get('pair').disabled, false);
  assert.equal(app.get('pair-code').value, code);
  assert.equal(app.get('pairing').hidden, false);
  assert.equal(app.get('manual-pairing').open, false);
  assert.equal(app.requests.length, 0, 'scan must not consume the one-use token');
  await app.get('pair').onclick();
  assert.equal(JSON.parse(app.requests[0].options.body).code, code);
  assert.equal(app.requests[0].options.headers.has('Authorization'), false);
  assert.deepEqual(app.requests.map(r => r.path), ['/api/pair', '/api/manifest', '/api/quickplay']);
  assert.equal(app.get('pairing').hidden, true);
  assert.equal(app.get('library').hidden, false);
  assert.equal(app.sessions.has('gnune-pair'), false);
  assert.equal(app.stores.settings.get('device').token, 'new');
});

test('scanned token survives a reload in its tab until paired', async () => {
  const app = await companion({savedCode: code});
  assert.equal(app.get('pair-code').value, code);
  assert.equal(app.get('pairing').hidden, false);
  assert.equal(app.requests.length, 0);
});

test('returning paired listener can decline a newly scanned queue', async () => {
  const app = await companion({scan: code, paired: true});
  assert.equal(app.get('pairing').hidden, false);
  assert.equal(app.get('cancel-pair').hidden, false);
  app.get('cancel-pair').onclick();
  await tick();
  assert.equal(app.get('pairing').hidden, true);
  assert.equal(app.stores.settings.get('device').token, 'old');
  assert.equal(app.requests.length, 0);
});

test('manual link fallback pairs and returning offline listener retains cached queue', async () => {
  const manual = await companion();
  assert.equal(manual.get('manual-pairing').open, true);
  manual.get('pair-code').value = `https://gnune.test/#pair=${code}`;
  await manual.get('pair').onclick();
  assert.equal(manual.stores.settings.get('device').token, 'new');
  const returning = await companion({paired: true, offline: true});
  await tick();
  assert.equal(returning.get('pairing').hidden, true);
  assert.equal(returning.get('library').hidden, false);
  assert.match(returning.get('status').textContent, /Working offline/);
});


test('browsing and repeated playlist queueing transfer no media; play fetches only current song', async () => {
  const data = {...queue, mode: 'library', tracks: [...queue.tracks, {...queue.tracks[0], id: 'second', title: 'Second'}], playlists: [{id: 'list', name: 'Repeated', tracks: ['song', 'second', 'song']}]};
  const app = await companion({paired: true, catalogData: data});
  const readsBefore = app.reads.filter(read => read.store === 'audio').length;
  vm.runInContext("navigate({pivot:'playlists', detail:null}); navigate({detail:{type:'playlist',id:'list'}}); addQueue(visible().items.map(track=>track.id))", app.context);
  assert.equal(app.requests.filter(request => request.path.startsWith('/api/media/')).length, 0);
  assert.equal(app.reads.filter(read => read.store === 'audio').length, readsBefore);
  assert.deepEqual(Array.from(vm.runInContext('playQueue', app.context)), ['song', 'second', 'song']);
  await vm.runInContext('play(0)', app.context);
  assert.deepEqual(app.requests.filter(request => request.path.startsWith('/api/media/')).map(request => request.path), ['/api/media/song']);
  assert.equal(app.stores.audio.size, 1);
  assert.equal(app.stores.audio.has('album:second:1'), false);
  await vm.runInContext('play(2)', app.context);
  assert.equal(app.requests.filter(request => request.path.startsWith('/api/media/')).length, 1, 'repeated song reuses its saved blob');
});

test('resume cookie recovers original credential and raises progress sequence floor', async () => {
  const app = await companion({cookie: true});
  assert.equal(app.stores.settings.get('device').token, 'old');
  assert.equal(app.stores.settings.get('sequence:old'), 20);
  assert.equal(app.requests.some(request => request.path === '/api/pair'), false);
  assert.equal(app.get('pairing').hidden, true);
});

test('revocation exposes pairing while preserving cached music for offline use', async () => {
  const app = await companion({paired: true, revoked: true});
  assert.equal(app.get('pairing').hidden, false);
  assert.equal(app.get('library').hidden, false);
  assert.match(app.get('status').textContent, /revoked/);
});

test('video browsing has no source or byte fetch; deliberate play starts native route only', async () => {
  const data = {...queue, mode: 'library', includes_video: true, videos: [{id: 'clip', title: 'Clip', artist: '', album: ''}]};
  const app = await companion({paired: true, catalogData: data});
  vm.runInContext("navigate({pivot:'videos',detail:null})", app.context);
  assert.equal(app.get('video').src, '');
  assert.equal(app.requests.some(request => request.path.startsWith('/api/video')), false);
  await vm.runInContext("playVideo(0,['clip'])", app.context);
  assert.equal(app.get('video').src, '/api/video/clip');
  assert.equal(app.requests.filter(request => request.path === '/api/video-session').length, 1);
  assert.equal(app.requests.some(request => request.path.startsWith('/api/media/')), false);
});


test('an already-open cookie-only offline app reconnects without pairing or media transfer', async () => {
  const app = await companion({cookie: true, offline: true});
  assert.equal(app.stores.settings.has('device'), false);
  app.setOnline(true);
  await vm.runInContext('reconnect()', app.context);
  assert.equal(app.stores.settings.get('device').token, 'old');
  assert.equal(app.stores.settings.get('sequence:old'), 20);
  assert.equal(app.get('pairing').hidden, true);
  assert.equal(app.requests.some(request => request.path === '/api/pair' || request.path.startsWith('/api/media/')), false);
});


test('installed queue app can change to a new library pairing without deleting old audio or pending progress', async () => {
  const blob = new Blob(['saved audio']);
  const pending = {key: 'album:song', token: 'old', seq: 3, id: 'song', position: 12, completed: false};
  const library = {...queue, scope: 'current-library', mode: 'library'};
  const app = await companion({paired: true, offline: true, pairingCatalog: library,
    savedAudio: [['album:song:1', blob]], savedPending: [pending]});
  const oldCredential = app.stores.settings.get('device');
  await app.get('change-pairing').onclick();
  assert.equal(app.get('pairing').hidden, false);
  assert.equal(app.get('manual-pairing').open, true);
  assert.equal(app.get('cancel-pair').hidden, false);
  const before = app.requests.length;
  app.setOnline(true);
  await vm.runInContext('reconnect()', app.context);
  assert.equal(app.requests.length, before, 'foreground recovery must respect explicit change intent');
  await app.get('pair').onclick();
  assert.equal(app.stores.settings.get('device'), oldCredential, 'empty input keeps the current credential');
  assert.equal(app.get('cancel-pair').hidden, false);
  await app.get('cancel-pair').onclick();
  assert.equal(app.get('pairing').hidden, true);
  await app.get('change-pairing').onclick();
  app.get('pair-code').value = 'invalid';
  await app.get('pair').onclick();
  assert.equal(app.stores.settings.get('device'), oldCredential, 'invalid input keeps the current credential');
  await app.get('cancel-pair').onclick();
  assert.equal(app.get('pairing').hidden, true);
  await app.get('change-pairing').onclick();
  app.get('pair-code').value = `https://gnune.test/#pair=${code}`;
  await app.get('pair').onclick();
  assert.equal(app.stores.settings.get('device').scope, 'current-library');
  assert.equal(app.stores.settings.get('manifest').mode, 'library');
  assert.equal(app.get('pairing').hidden, true);
  assert.equal(app.stores.audio.get('album:song:1'), blob);
  assert.equal(app.stores.pending.get('album:song'), pending);
  assert.equal(app.requests.some(request => request.path === '/api/forget' || request.path.startsWith('/api/media/')), false);
});


const phoneShared = () => ({pins:[{id:'record',track:'song'}],new:[{id:'record',track:'song'}],history:[],progress:{song:{position:42,completed:false,revision:'r42'}},includes_podcasts:true,podcasts:[{id:'show',title:'Radio',summary:'A show',order:'newest_first'}],last_podcast:{show:'show',episode:'first'},last_revision:'last0'});
const phoneEpisodes = () => ({id:'show',order:'newest_first',episodes:[{id:'first',show:'show',title:'First',published:20,progress:{position:32,completed:false,revision:'r32'}},{id:'second',show:'show',title:'Second',published:10,progress:{position:0,completed:false,revision:'r0'}}]});

test('shared Quickplay browsing and podcast episode listing transfer no media; play selects only one trusted session', async()=>{
  const app=await companion({paired:true,catalogData:{...queue,mode:'library'},sharedData:phoneShared(),podcastData:phoneEpisodes()});
  await vm.runInContext("openPodcast('show')",app.context);
  assert.equal(app.requests.some(r=>r.path==='/api/podcast-session'),false);
  assert.equal(app.get('audio').src,'');
  await vm.runInContext("playPodcast(0,['first','second'])",app.context);
  assert.equal(app.get('audio').src,'/api/podcast-media/episode');
  assert.equal(app.get('audio').currentTime,32);
  assert.equal(app.stores.audio.size,0,'streaming podcasts never fill song Blob cache');
  assert.equal(app.requests.filter(r=>r.path==='/api/podcast-session').length,1);
  assert.equal(app.requests.some(r=>r.path.startsWith('http')),false);
});

test('podcast queue survives browsing another show; EOS skips completed and rewind preserves paused state', async()=>{
  const data=phoneEpisodes(); data.episodes.push({id:'third',show:'show',title:'Third',progress:{position:7,completed:false,revision:'r7'}});data.episodes[1].progress.completed=true;
  const app=await companion({paired:true,catalogData:{...queue,mode:'library'},sharedData:phoneShared(),podcastData:data});
  await vm.runInContext("openPodcast('show');",app.context);
  await vm.runInContext("playPodcast(0,['first','second','third'])",app.context);
  vm.runInContext("navigate({area:'music',detail:null})",app.context);
  await vm.runInContext('adjacent(1,true)',app.context);
  assert.equal(vm.runInContext('currentPodcast.id',app.context),'third');
  app.get('audio').currentTime=7;app.get('audio').paused=true;
  await vm.runInContext('rewind()',app.context);
  assert.equal(app.get('audio').currentTime,0);
  assert.equal(app.get('audio').paused,true);
});

test('mobile rewind synchronizes music/video positions and keeps video data out of offline audio cache',async()=>{
  const shared=phoneShared();shared.progress.clip={position:22,completed:false,revision:'r22'};
  const app=await companion({paired:true,catalogData:{...queue,mode:'library',includes_video:true,videos:[{id:'clip',title:'Clip'}]},sharedData:shared});
  await vm.runInContext("playVideo(0,['clip'])",app.context);
  assert.equal(app.get('video').currentTime,22);
  app.get('video').currentTime=22;app.get('video').paused=true;
  await vm.runInContext('rewind()',app.context);
  assert.equal(app.get('video').currentTime,12);
  assert.equal(app.get('video').paused,true);
  assert.equal(app.stores.audio.size,0);
  assert.ok(app.requests.some(r=>r.path==='/api/state'&&JSON.parse(r.options.body).id==='clip'));
});

test('a conflicting offline position cannot silently replace newer shared progress',async()=>{
  const app=await companion({paired:true,catalogData:{...queue,mode:'library'},sharedData:phoneShared(),rejectState:true});
  await vm.runInContext("play(0,['song'])",app.context);
  await tick();await tick();
  app.get('audio').currentTime=10;
  await vm.runInContext('sync()',app.context);
  assert.equal(vm.runInContext('currentTrack.conflicted',app.context),true);
  assert.equal(app.stores.pending.size,0);
});


test('modern revision survives repeated lightweight polls and conflict recovers on deliberate reopen',async()=>{
  const app=await companion({paired:true,catalogData:{...queue,mode:'library'},sharedData:phoneShared(),rejectState:true});
  await vm.runInContext("play(0,['song'])",app.context);
  await tick();await tick();
  await vm.runInContext('sync()',app.context);
  assert.match(app.get('status').textContent,/newer listening position/);
  app.setRejectState(false);
  await vm.runInContext("play(0,['song'])",app.context);
  await tick();await tick();
  await vm.runInContext('refreshShared();',app.context);
  app.get('audio').currentTime=40;
  await vm.runInContext('sync()',app.context);
  assert.equal(vm.runInContext('currentTrack.conflicted',app.context),false);
  assert.equal(app.requests.some(r=>r.path==='/api/progress'),false,'new phone edits must keep CAS across polls');
});

test('a slow earlier podcast open cannot replace a later selected song',async()=>{
  const app=await companion({paired:true,catalogData:{...queue,mode:'library'},sharedData:phoneShared(),podcastData:phoneEpisodes()});
  vm.runInContext("const originalFetch=fetch;let releaseShow;fetch=(path,options)=>path.startsWith('/api/podcast-show/')?new Promise(resolve=>{releaseShow=()=>originalFetch(path,options).then(resolve);}):originalFetch(path,options);",app.context);
  const opened=vm.runInContext("openPodcast('show','latest')",app.context);
  for(let i=0;i<20&&!vm.runInContext('Boolean(releaseShow)',app.context);i++)await tick();
  await vm.runInContext("play(0,['song'])",app.context);
  vm.runInContext('releaseShow()',app.context);await opened;
  assert.equal(vm.runInContext('currentTrack.id',app.context),'song');
  assert.equal(vm.runInContext('currentPodcast',app.context),undefined);
  assert.equal(app.requests.some(r=>r.path==='/api/podcast-session'),false);
});

test('completed last-episode Quickplay resume wraps to an earlier unfinished episode',async()=>{
  const episodes=phoneEpisodes();episodes.episodes[1].progress.completed=true;
  const app=await companion({paired:true,catalogData:{...queue,mode:'library'},sharedData:phoneShared(),podcastData:episodes});
  await vm.runInContext("openPodcast('show','second')",app.context);
  assert.equal(vm.runInContext('currentPodcast.id',app.context),'first');
  assert.equal(app.get('audio').currentTime,32);
});

test('clearing a manifest on pairing replacement clears podcast capability and active queue state',async()=>{
  const app=await companion({paired:true,catalogData:{...queue,mode:'library'},sharedData:phoneShared(),podcastData:phoneEpisodes()});
  await vm.runInContext("openPodcast('show')",app.context);
  await vm.runInContext("playPodcast(0,['first','second'])",app.context);
  vm.runInContext('stopPlayback();acceptManifest(undefined)',app.context);
  assert.equal(vm.runInContext('sharedState',app.context),undefined);
  assert.equal(vm.runInContext('podcastShows.size',app.context),0);
  assert.equal(vm.runInContext('activePodcastShow',app.context),undefined);
  assert.equal(vm.runInContext('podcastQueue.length',app.context),0);
  assert.equal(app.stores.settings.get('podcastPlayback').token,'old');
});
