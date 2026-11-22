// Run with: node tests/mobile-catalog.cjs
const assert = require('node:assert/strict');
const test = require('node:test');
const Catalog = require('../src/mobile/catalog.js');
const tracks = [
  {id: 'b', title: 'Second', artist: 'Singer B', album_artist: 'Various Artists', album: 'Mix', album_id: 'release-one', disc_number: 1, track_number: 2},
  {id: 'a', title: 'First', artist: 'Singer A', album_artist: 'Various Artists', album: 'Mix', album_id: 'release-one', disc_number: 1, track_number: 1},
  {id: 'c', title: 'Other release', artist: 'Singer A', album_artist: 'Various Artists', album: 'Mix', album_id: 'release-two', disc_number: 2, track_number: 1},
];
const model = Catalog.build({tracks, playlists: [{id: 'p', name: 'My order', tracks: ['b', 'a', 'b', 'missing']}]});
test('desktop album IDs keep compilations together and distinct releases apart', () => {
  assert.equal(model.albums.size, 2);
  assert.deepEqual(model.albums.get('release-one').tracks.map(track => track.id), ['a', 'b']);
  const artist = Catalog.visible(model, {pivot: 'artists', detail: {type: 'artist', id: 'Singer A'}}, () => true);
  assert.equal(artist.items.length, 2);
});
test('saved playlist order and repeats survive grouping and offline filtering', () => {
  const view = {pivot: 'playlists', detail: {type: 'playlist', id: 'p'}};
  assert.deepEqual(Catalog.visible(model, view, () => true).items.map(track => track.id), ['b', 'a', 'b']);
  assert.deepEqual(Catalog.visible(model, {...view, offline: true}, track => track.id === 'b').items.map(track => track.id), ['b', 'b']);
});
test('artist album drilldown and search preserve individual song credits', () => {
  const view = {pivot: 'artists', detail: {type: 'album', id: 'release-one', artist: 'Singer A'}, search: 'first'};
  assert.deepEqual(Catalog.visible(model, view, () => true).items.map(track => track.id), ['a']);
});
