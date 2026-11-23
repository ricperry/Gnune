use super::*;

impl Ui {
    pub(super) fn quickplay_page(self: &Rc<Self>) {
        let (pins, added, history) = {
            let db = self.db.borrow();
            let lookup = db.tracks.iter().map(|t| (t.path.as_path(), t)).collect();
            (
                recent_albums(&lookup, &db.pins, 5),
                recent_albums(&lookup, &db.added, 7),
                recent_albums(&lookup, &db.history, 4),
            )
        };
        let (tier, large, _) = self.quickplay_dimensions();
        self.quickplay_tier.set(tier);
        let page = col(32);
        page.add_css_class("quickplay-page");
        page.set_valign(gtk::Align::Start);
        let sections = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .min_children_per_line(1)
            .max_children_per_line(5)
            .column_spacing(32)
            .row_spacing(28)
            .build();
        sections.add_css_class("quickplay-sections");
        sections.set_valign(gtk::Align::Start);
        page.append(&sections);
        let pinned = section("pins", "quickplay-pins", large * 2 + 8);
        sections.insert(&pinned, -1);
        if pins.is_empty() {
            pinned.append(&quick_text("Right-click a sleeve to pin its album."));
        } else {
            self.quickplay_mosaic(&pinned, &pins, TileAction::Album);
        }

        let categories = section("collection", "quickplay-welcome", large.max(180));
        for (name, destination, icon_name) in [
            ("MUSIC", "artists", "folder-music-symbolic"),
            ("VIDEOS", "videos", "video-x-generic-symbolic"),
            ("PODCASTS", "podcasts", "audio-x-generic-symbolic"),
        ] {
            let category = app_button(self, "", move |ui| {
                ui.go(Page::Collection(destination.into()))
            });
            category.add_css_class("quickplay-category");
            let line = row(10);
            let image = gtk::Image::from_icon_name(icon_name);
            image.set_pixel_size(28);
            line.append(&image);
            line.append(&label(name, "quickplay-action-label"));
            category.set_child(Some(&line));
            categories.append(&category);
        }
        categories.append(&app_button(self, "ADD MUSIC", |ui| ui.choose(true)));
        sections.insert(&categories, -1);
        let podcast = section("latest podcast", "quickplay-podcast", large.max(180));
        self.quickplay_podcast(&podcast, large.max(164));
        sections.insert(&podcast, -1);
        let new = section("new", "quickplay-new", large * 2 + 8);
        sections.insert(&new, -1);
        if added.is_empty() {
            new.append(&quick_text("Newly added albums appear here."));
        } else {
            self.quickplay_mosaic(&new, &added, TileAction::New);
        }
        let recent = section("history", "quickplay-history", large * 2 + 8);
        sections.insert(&recent, -1);
        if history.is_empty() {
            recent.append(&quick_text("Play some music to start your history."));
        } else {
            self.quickplay_mosaic(&recent, &history, TileAction::History);
        }

        let dj = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .min_children_per_line(1)
            .max_children_per_line(6)
            .column_spacing(8)
            .row_spacing(8)
            .build();
        dj.add_css_class("quickplay-smart-dj");
        dj.set_margin_start(26);
        dj.set_margin_end(26);
        if self.smart_dj_controls(&dj, &history, &added) {
            page.append(&dj);
        }
        let show_shelf = col(10);
        show_shelf.set_margin_start(26);
        show_shelf.set_margin_end(26);
        show_shelf.append(&label("your podcasts", "quickplay-section-title"));
        if self.quickplay_podcast_shelf(&show_shelf, large.clamp(148, 260)) {
            page.append(&show_shelf);
        }
        let video_shelf = col(10);
        video_shelf.set_margin_start(26);
        video_shelf.set_margin_end(26);
        video_shelf.append(&label("videos", "quickplay-section-title"));
        if self.quickplay_video_shelf(&video_shelf, (large * 3 / 2).clamp(188, 340)) {
            page.append(&video_shelf);
        }
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&page));
        self.body.append(&scroll);
    }

    fn smart_dj_controls(
        self: &Rc<Self>,
        dj: &gtk::FlowBox,
        history: &[Track],
        added: &[Track],
    ) -> bool {
        let (seeds, more) = smart_dj_seeds(
            &self.db.borrow().tracks,
            history,
            added,
            &mut self.smart_dj_rotation.borrow_mut(),
        );
        while let Some(child) = dj.first_child() {
            dj.remove(&child);
        }
        let has_artists = !seeds.is_empty();
        for track in seeds {
            let name = track.artist.clone();
            let b = app_button(self, "", move |ui| ui.mix(&track));
            b.add_css_class("quickplay-dj-seed");
            let names = col(2);
            let artist = label(&name, "quickplay-dj-artist");
            artist.set_max_width_chars(14);
            artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
            names.append(&artist);
            names.append(&label("smart dj", "quickplay-dj-label"));
            b.set_tooltip_text(Some(&format!("Start Smart DJ from {name}")));
            b.set_child(Some(&names));
            dj.insert(&b, -1);
        }
        if more {
            let row = dj.downgrade();
            let history = history.to_vec();
            let added = added.to_vec();
            let more = app_button(self, "More artists", move |ui| {
                ui.smart_dj_rotation.borrow_mut().request_next();
                if let Some(row) = row.upgrade() {
                    ui.smart_dj_controls(&row, &history, &added);
                }
            });
            more.set_tooltip_text(Some("Show another group of Smart DJ artists"));
            more.add_css_class("quickplay-dj-seed");
            dj.insert(&more, -1);
        }
        has_artists
    }

    pub(crate) fn quickplay_dimensions(&self) -> (u8, i32, i32) {
        let width = self.win.width().max(700);
        let tier = ((width - 1000).max(0) / 240).min(20) as u8;
        let large = if width < 1280 {
            120
        } else {
            (width / 10).clamp(140, 360)
        };
        (tier, large, (large - 4) / 2)
    }

    fn quickplay_mosaic(self: &Rc<Self>, section: &gtk::Box, tracks: &[Track], action: TileAction) {
        let (_, large, small) = self.quickplay_dimensions();
        let grid = gtk::Grid::new();
        grid.add_css_class("quickplay-mosaic");
        grid.set_column_spacing(4);
        grid.set_row_spacing(4);
        grid.set_row_homogeneous(true);
        grid.set_column_homogeneous(true);
        grid.set_halign(gtk::Align::Start);
        section.append(&grid);
        for (index, track) in tracks.iter().enumerate() {
            let (x, y, span, size) = if index == 0 {
                (0, 0, 2, large)
            } else {
                let position = index as i32 - 1;
                if action == TileAction::New {
                    (2 + position % 2, position / 2, 1, small)
                } else {
                    (position % 2, 2 + position / 2, 1, small)
                }
            };
            let tile = gtk::Button::new();
            tile.add_css_class("quickplay-tile");
            tile.set_halign(gtk::Align::Start);
            tile.set_valign(gtk::Align::Start);
            // Paintable pixels are device-sized. Do not let their intrinsic
            // resolution stretch logical mosaic cells on a scaled display.
            let cover = gtk::Overlay::new();
            cover.set_child(Some(&visual::picture(None, size)));
            if let Some(path) = self.cover(track) {
                let image = visual::picture(Some(&path), size);
                cover.add_overlay(&image);
                cover.set_measure_overlay(&image, false);
            }
            tile.set_child(Some(&cover));
            tile.set_tooltip_text(Some(&format!(
                "{} — {}\n{}\nRight-click for album actions",
                track.album_artist,
                track.album,
                if action == TileAction::Album {
                    "Open album"
                } else {
                    "Play album"
                }
            )));
            tile.update_property(&[gtk::accessible::Property::Label(&format!(
                "{} — {}",
                track.album_artist, track.album
            ))]);
            let u = self.clone();
            let t = track.clone();
            tile.connect_clicked(move |_| match action {
                TileAction::Album => u.go(Page::Album(album_key(&t))),
                TileAction::New | TileAction::History => {
                    u.quickplay_start(&t, action == TileAction::History)
                }
            });
            let u = self.clone();
            let t = track.clone();
            tile_context_menu(&tile, move |tile| u.quickplay_menu(tile, &t));
            grid.attach(&tile, x, y, span, span);
        }
    }

    fn quickplay_start(self: &Rc<Self>, seed: &Track, history: bool) {
        let key = album_key(seed);
        let mut album: Vec<_> = self
            .db
            .borrow()
            .tracks
            .iter()
            .filter(|t| album_key(t) == key)
            .cloned()
            .collect();
        sort_album_tracks(&mut album);
        let index = if history {
            album.iter().position(|t| t.path == seed.path).unwrap_or(0)
        } else {
            0
        };
        self.start(album, index, None);
    }

    pub(super) fn quickplay_menu(self: &Rc<Self>, tile: &gtk::Button, track: &Track) {
        close_tile_menu(tile);
        let pop = gtk::Popover::new();
        pop.set_parent(tile);
        let actions = col(2);
        let pinned = self.album_is_pinned(track);
        for title in [
            "Play album",
            "Now playing",
            "Album details",
            "Smart DJ",
            if pinned { "Unpin" } else { "Pin to Quickplay" },
        ] {
            let b = gtk::Button::with_label(title);
            if title == "Now playing" {
                b.set_sensitive(
                    self.current()
                        .as_ref()
                        .is_some_and(|current| album_key(current) == album_key(track)),
                );
                b.set_tooltip_text(Some("Follow this playing album into the listening view"));
            }
            let source = tile.child();
            let u = self.clone();
            let t = track.clone();
            let weak_pop = pop.downgrade();
            b.connect_clicked(move |_| {
                if let Some(pop) = weak_pop.upgrade() {
                    pop.popdown();
                    // Navigation can remove the anchor before its close animation ends.
                    if pop.parent().is_some() {
                        pop.unparent();
                    }
                }
                match title {
                    "Play album" => u.quickplay_start(&t, false),
                    "Now playing" => {
                        if let Some(source) = source.as_ref() {
                            u.motion.set_art_source(source, u.cover(&t).as_deref());
                        }
                        u.go(Page::Now)
                    }
                    "Album details" => u.go(Page::Album(album_key(&t))),
                    "Smart DJ" => u.mix(&t),
                    _ => {
                        u.set_album_pinned(&t, !pinned);
                        u.persist();
                        if *u.page.borrow() == Page::Quick {
                            u.render();
                        }
                    }
                }
            });
            actions.append(&b);
        }
        pop.set_child(Some(&actions));
        pop.connect_closed(|pop| {
            if pop.parent().is_some() {
                pop.unparent();
            }
        });
        pop.popup();
    }

    pub(super) fn album_is_pinned(&self, track: &Track) -> bool {
        let db = self.db.borrow();
        let key = album_key(track);
        let pins: std::collections::HashSet<_> = db.pins.iter().collect();
        db.tracks
            .iter()
            .any(|t| pins.contains(&t.path) && album_key(t) == key)
    }

    pub(super) fn set_album_pinned(&self, track: &Track, pinned: bool) {
        set_album_pin(&mut self.db.borrow_mut(), track, pinned);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum TileAction {
    Album,
    New,
    History,
}

fn section(title: &str, class: &str, width: i32) -> gtk::Box {
    let section = col(8);
    section.add_css_class(class);
    section.set_size_request(width, -1);
    section.set_valign(gtk::Align::Start);
    section.append(&label(title, "quickplay-section-title"));
    section
}

fn quick_text(text: &str) -> gtk::Label {
    let l = label(text, "quickplay-copy");
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    l.set_ellipsize(gtk::pango::EllipsizeMode::None);
    l.set_width_chars(18);
    l.set_max_width_chars(23);
    l
}

fn recent_albums(lookup: &HashMap<&Path, &Track>, paths: &[PathBuf], limit: usize) -> Vec<Track> {
    let mut seen = BTreeSet::new();
    paths
        .iter()
        .filter_map(|p| lookup.get(p.as_path()).copied())
        .filter(|t| seen.insert(album_key(t)))
        .take(limit)
        .cloned()
        .collect()
}

fn set_album_pin(db: &mut Library, track: &Track, pinned: bool) {
    let key = album_key(track);
    let paths: Vec<_> = db
        .tracks
        .iter()
        .filter(|t| album_key(t) == key)
        .map(|t| t.path.clone())
        .collect();
    let matching: std::collections::HashSet<_> = paths.iter().collect();
    db.pins.retain(|p| !matching.contains(p));
    if pinned && let Some(canonical) = paths.first() {
        state::promote(&mut db.pins, canonical);
    }
}

#[derive(Default)]
pub(crate) struct SmartDjRotation {
    artists: Vec<String>,
    remaining: Vec<String>,
    current: Vec<String>,
    advance: bool,
}
impl SmartDjRotation {
    pub(crate) fn request_next(&mut self) {
        self.advance = true;
    }

    fn choose(&mut self, artists: Vec<String>, rng: &mut impl rand::Rng) -> &[String] {
        if self.artists != artists {
            self.current.retain(|artist| artists.contains(artist));
            self.remaining = artists
                .iter()
                .filter(|artist| !self.current.contains(artist))
                .cloned()
                .collect();
            self.remaining.shuffle(rng);
            self.artists = artists;
        }
        if self.artists.len() <= 4 {
            self.current = self.artists.clone();
            self.remaining.clear();
        } else if self.advance || self.current.len() != 4 {
            let mut group = Vec::with_capacity(4);
            let mut deferred = Vec::new();
            while group.len() < 4 {
                if self.remaining.is_empty() {
                    self.remaining = self.artists.clone();
                    self.remaining.shuffle(rng);
                }
                let artist = self.remaining.pop().unwrap();
                if group.contains(&artist) {
                    deferred.push(artist);
                } else {
                    group.push(artist);
                }
            }
            // Artists already shown at the tail of the old pass still belong
            // to the new pass, but cannot appear twice in this group.
            self.remaining.extend(deferred);
            self.current = group;
        }
        self.advance = false;
        &self.current
    }
}

fn smart_dj_seeds(
    tracks: &[Track],
    history: &[Track],
    added: &[Track],
    rotation: &mut SmartDjRotation,
) -> (Vec<Track>, bool) {
    let mut artists = std::collections::BTreeMap::new();
    for track in history.iter().chain(added).chain(tracks) {
        if !track.artist.trim().is_empty() {
            artists.entry(track.artist.clone()).or_insert(track);
        }
    }
    let more = artists.len() > 4;
    let selection = rotation.choose(artists.keys().cloned().collect(), &mut rand::rng());
    (
        selection
            .iter()
            .filter_map(|artist| artists.get(artist).map(|track| (*track).clone()))
            .collect(),
        more,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{SeedableRng, rngs::StdRng};

    #[test]
    fn smart_dj_visits_each_artist_before_repeating_and_never_duplicates_a_group() {
        for count in 5..=13 {
            let artists: Vec<_> = (0..count).map(|i| format!("Artist {i:02}")).collect();
            let mut rotation = SmartDjRotation::default();
            let mut rng = StdRng::seed_from_u64(count as u64);
            let mut visited = BTreeSet::new();
            for _ in 0..count * 4 {
                rotation.request_next();
                let group = rotation.choose(artists.clone(), &mut rng).to_vec();
                assert_eq!(group.len(), 4);
                assert_eq!(group.iter().collect::<BTreeSet<_>>().len(), 4);
                for artist in &group {
                    if visited.len() == count {
                        visited.clear();
                    }
                    assert!(
                        visited.insert(artist.clone()),
                        "repeat before visiting pool"
                    );
                }
                // Incidental redraws must not consume the bag or change the group.
                let remaining = rotation.remaining.clone();
                assert_eq!(rotation.choose(artists.clone(), &mut rng), group);
                assert_eq!(rotation.remaining, remaining);
            }
        }
    }

    #[test]
    fn smart_dj_small_pools_remain_complete_and_stable() {
        let mut rng = StdRng::seed_from_u64(1);
        for count in 0..=4 {
            let artists: Vec<_> = (0..count).map(|i| format!("Artist {i}")).collect();
            let mut rotation = SmartDjRotation::default();
            for _ in 0..8 {
                rotation.request_next();
                assert_eq!(rotation.choose(artists.clone(), &mut rng), artists);
            }
        }
    }

    #[test]
    fn smart_dj_uses_distinct_nonempty_local_artists_and_retains_the_group() {
        let tracks: Vec<_> = ["One", "Two", "One", "", " ", "Three", "Four", "Five"]
            .iter()
            .enumerate()
            .map(|(i, artist)| Track {
                path: format!("track{i}").into(),
                artist: (*artist).into(),
                ..Track::default()
            })
            .collect();
        let mut rotation = SmartDjRotation::default();
        let (first, more) = smart_dj_seeds(&tracks, &[], &[], &mut rotation);
        assert!(more);
        assert_eq!(first.len(), 4);
        assert_eq!(
            first
                .iter()
                .map(|t| &t.artist)
                .collect::<BTreeSet<_>>()
                .len(),
            4
        );
        assert!(first.iter().all(|track| tracks.contains(track)));
        let (again, _) = smart_dj_seeds(&tracks, &tracks[1..2], &[], &mut rotation);
        assert_eq!(
            first.iter().map(|t| &t.artist).collect::<Vec<_>>(),
            again.iter().map(|t| &t.artist).collect::<Vec<_>>()
        );
        let (small, more) = smart_dj_seeds(&tracks[..3], &[], &[], &mut rotation);
        assert!(!more);
        assert_eq!(small.len(), 2);
    }

    #[test]
    fn mosaics_keep_recent_album_order_and_skip_missing_and_duplicate_paths() {
        let a = Track {
            path: "a".into(),
            album: "First".into(),
            album_artist: "Artist".into(),
            ..Track::default()
        };
        let b = Track {
            path: "b".into(),
            ..a.clone()
        };
        let c = Track {
            path: "c".into(),
            album: "Second".into(),
            ..a.clone()
        };
        let tracks = [a, b.clone(), c.clone()];
        let lookup = tracks.iter().map(|t| (t.path.as_path(), t)).collect();
        let result = recent_albums(
            &lookup,
            &["missing".into(), "c".into(), "b".into(), "a".into()],
            8,
        );
        assert_eq!(result, vec![c, b]);
    }

    #[test]
    fn pinning_a_later_track_canonicalizes_and_unpinning_removes_the_release() {
        let a = Track {
            path: "first".into(),
            release_id: Some("release".into()),
            ..Track::default()
        };
        let b = Track {
            path: "later".into(),
            album: "Other spelling".into(),
            ..a.clone()
        };
        let mut db = Library {
            tracks: vec![a.clone(), b.clone()],
            pins: vec!["unrelated".into(), b.path.clone()],
            ..Library::default()
        };
        set_album_pin(&mut db, &b, true);
        assert_eq!(db.pins, vec![a.path.clone(), "unrelated".into()]);
        set_album_pin(&mut db, &a, false);
        assert_eq!(db.pins, vec![PathBuf::from("unrelated")]);
    }
}
