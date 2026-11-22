'use strict';
// Pure catalog operations shared by the touch UI and its regression tests.
globalThis.GnuneCatalog = (() => {
  const compare = (a, b) => String(a || '').localeCompare(String(b || ''), undefined, {sensitivity: 'base', numeric: true});
  const albumId = track => track.album_id || JSON.stringify([track.album_artist || track.artist, track.album]);
  const ordered = tracks => [...tracks].sort((a, b) =>
    (a.disc_number || 1) - (b.disc_number || 1)
    || (a.track_number || 0) - (b.track_number || 0)
    || compare(a.title, b.title));
  function build(manifest) {
    const tracks = new Map(), albums = new Map(), artists = new Map();
    for (const track of manifest?.tracks || []) {
      tracks.set(track.id, track);
      const id = albumId(track), artist = track.artist || 'Unknown artist';
      if (!albums.has(id)) {
        albums.set(id, {
          id, title: track.album || 'Unknown album', artist: track.album_artist || artist,
          tracks: [], art: track.art,
        });
      }
      albums.get(id).tracks.push(track);
      if (!artists.has(artist)) artists.set(artist, {id: artist, title: artist, tracks: [], albums: new Set()});
      artists.get(artist).tracks.push(track);
      artists.get(artist).albums.add(id);
    }
    for (const album of albums.values()) album.tracks = ordered(album.tracks);
    const playlists = new Map((manifest?.playlists || []).map(list => [list.id, {
      ...list, title: list.name, tracks: list.tracks.map(id => tracks.get(id)).filter(Boolean),
    }]));
    const videos = new Map((manifest?.videos || []).map(video => [video.id, video]));
    const videoPlaylists = new Map((manifest?.video_playlists || []).map(list => [list.id, {
      ...list, title: list.name, tracks: list.tracks.map(id => videos.get(id)).filter(Boolean),
    }]));
    return {tracks, albums, artists, playlists, videos, videoPlaylists};
  }
  const matches = (value, search) => !search || String(value).toLocaleLowerCase().includes(search.toLocaleLowerCase());
  function visible(catalog, view, available, queue = []) {
    const eligible = track => !view.offline || available(track);
    const detail = view.detail;
    let kind = 'tracks', title = 'songs', subtitle = '', items = [];
    if (detail?.type === 'video-playlist') {
      kind = 'videos'; title = catalog.videoPlaylists.get(detail.id)?.title || 'video playlist';
      items = catalog.videoPlaylists.get(detail.id)?.tracks || [];
    } else if (detail?.type === 'video-queue') {
      kind = 'videos'; title = 'up next'; items = queue.map(id => catalog.videos.get(id)).filter(Boolean);
    } else if (detail?.type === 'album') {
      const album = catalog.albums.get(detail.id);
      title = album?.title || 'album'; subtitle = album?.artist || '';
      items = (album?.tracks || []).filter(track => !detail.artist || track.artist === detail.artist);
    } else if (detail?.type === 'playlist') {
      const playlist = catalog.playlists.get(detail.id);
      title = playlist?.title || 'playlist'; items = playlist?.tracks || [];
    } else if (detail?.type === 'queue') {
      title = 'up next'; items = queue.map(id => catalog.tracks.get(id)).filter(Boolean);
    } else if (detail?.type === 'artist') {
      title = detail.id; kind = 'albums';
      items = [...(catalog.artists.get(detail.id)?.albums || [])].map(id => catalog.albums.get(id));
    } else if (view.pivot === 'videos') {
      kind = 'videos'; title = 'videos'; items = [...catalog.videos.values()].sort((a, b) => compare(a.title, b.title));
    } else if (view.pivot === 'video-playlists') {
      kind = 'video-playlists'; title = 'video playlists'; items = [...catalog.videoPlaylists.values()];
    } else if (view.pivot === 'artists') {
      title = 'artists'; kind = 'artists'; items = [...catalog.artists.values()];
    } else if (view.pivot === 'albums') {
      title = 'albums'; kind = 'albums'; items = [...catalog.albums.values()];
    } else if (view.pivot === 'playlists') {
      title = 'playlists'; kind = 'playlists'; items = [...catalog.playlists.values()];
    } else {
      items = [...catalog.tracks.values()].sort((a, b) => compare(a.title, b.title) || compare(a.artist, b.artist));
    }
    if (kind === 'videos') items = view.offline ? [] : items.filter(track => matches(`${track.title} ${track.artist} ${track.album}`, view.search));
    else if (kind === 'tracks') items = items.filter(track => eligible(track) && matches(`${track.title} ${track.artist} ${track.album}`, view.search));
    else {
      items = items.filter(item => item && (!view.offline || item.tracks.some(eligible)) && matches(`${item.title} ${item.artist || ''}`, view.search));
      items.sort((a, b) => compare(a.title, b.title) || compare(a.artist, b.artist));
    }
    return {kind, title, subtitle, items};
  }
  return {build, visible, ordered, albumId};
})();
if (typeof module !== 'undefined') module.exports = globalThis.GnuneCatalog;
