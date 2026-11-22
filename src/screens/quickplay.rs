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
        let page = col(32);
        page.add_css_class("quickplay-page");
        page.set_valign(gtk::Align::Start);
        let sections = row(30);
        sections.add_css_class("quickplay-sections");
        sections.set_valign(gtk::Align::Start);
        page.append(&sections);
        let pinned = section("pins", "quickplay-pins", 170);
        sections.append(&pinned);
        if pins.is_empty() {
            pinned.append(&quick_text("Right-click a sleeve to pin its album."));
        } else {
            self.quickplay_mosaic(&pinned, &pins, TileAction::Album);
        }

        let welcome = section("welcome", "quickplay-welcome", 190);
        sections.append(&welcome);
        welcome.append(&quick_text(
            "Play an album here, or browse your local collection.",
        ));
        let browse = app_button(self, "", |u| u.go(Page::Collection("artists".into())));
        browse.add_css_class("quickplay-collection-link");
        let content = col(6);
        let image = gtk::Image::from_icon_name("folder-music-symbolic");
        image.set_pixel_size(40);
        image.set_halign(gtk::Align::Start);
        content.append(&image);
        content.append(&label("GO TO YOUR\nCOLLECTION", "quickplay-action-label"));
        browse.set_child(Some(&content));
        browse.set_tooltip_text(Some("Browse your local music collection"));
        browse.update_property(&[gtk::accessible::Property::Label("Go to your collection")]);
        welcome.append(&browse);
        let add = app_button(self, "ADD MUSIC", |u| u.choose(true));
        add.add_css_class("quickplay-add");
        add.set_halign(gtk::Align::Start);
        welcome.append(&add);

        let new = section("new", "quickplay-new", 248);
        sections.append(&new);
        if added.is_empty() {
            new.append(&quick_text("Newly added albums appear here."));
        } else {
            self.quickplay_mosaic(&new, &added, TileAction::New);
        }
        let recent = section("history", "quickplay-history", 150);
        sections.append(&recent);
        if history.is_empty() {
            recent.append(&quick_text("Play some music to start your history."));
        } else {
            self.quickplay_mosaic(&recent, &history, TileAction::History);
        }

        let seeds = smart_dj_seeds(self.db.borrow().tracks.as_slice(), &history, &added);
        if !seeds.is_empty() {
            let dj = row(12);
            dj.add_css_class("quickplay-smart-dj");
            dj.set_margin_start(200);
            page.append(&dj);
            for track in seeds {
                let name = track.artist.clone();
                let b = app_button(self, "", move |u| u.mix(&track));
                b.add_css_class("quickplay-dj-seed");
                // The seed is an actual local artist; Smart DJ uses the existing matcher.
                let names = col(2);
                let artist = label(&name, "quickplay-dj-artist");
                artist.set_max_width_chars(14);
                artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
                names.append(&artist);
                b.set_tooltip_text(Some(&format!("Start Smart DJ from {name}")));
                names.append(&label("smart dj", "quickplay-dj-label"));
                b.set_child(Some(&names));
                dj.append(&b);
            }
        }
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&page));
        self.body.append(&scroll);
    }

    fn quickplay_mosaic(self: &Rc<Self>, section: &gtk::Box, tracks: &[Track], action: TileAction) {
        let grid = gtk::Grid::new();
        grid.add_css_class("quickplay-mosaic");
        grid.set_column_spacing(4);
        grid.set_row_spacing(4);
        grid.set_halign(gtk::Align::Start);
        section.append(&grid);
        for (index, track) in tracks.iter().enumerate() {
            let (x, y, span, size) = if index == 0 {
                (0, 0, 2, 120)
            } else {
                let position = index as i32 - 1;
                if action == TileAction::New {
                    (2 + position % 2, position / 2, 1, 58)
                } else {
                    (position % 2, 2 + position / 2, 1, 58)
                }
            };
            let tile = gtk::Button::new();
            tile.add_css_class("quickplay-tile");
            tile.set_halign(gtk::Align::Start);
            tile.set_valign(gtk::Align::Start);
            let cover = visual::picture(self.cover(track).as_deref(), size);
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

fn smart_dj_seeds(tracks: &[Track], history: &[Track], added: &[Track]) -> Vec<Track> {
    let mut artists = BTreeSet::new();
    history
        .iter()
        .chain(added)
        .chain(tracks)
        .filter(|t| artists.insert(t.artist.clone()))
        .take(4)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
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
