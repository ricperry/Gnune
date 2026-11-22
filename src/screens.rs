use super::*;
use std::collections::BTreeSet;
mod collection;
mod quickplay;
mod videos;
pub(crate) use collection::Browser;
pub(crate) use videos::VideoGallery;

#[derive(PartialEq)]
pub(super) struct ViewState {
    page: Page,
    listening_track: Option<String>,
    fullscreen_art: bool,
}

impl Ui {
    pub(super) fn render(self: &Rc<Self>) {
        let page = self.page.borrow().clone();
        let listening = matches!(page, Page::Now | Page::Wall | Page::Showcase);
        let view = ViewState {
            page: page.clone(),
            listening_track: if listening {
                self.current.borrow().as_ref().map(|t| t.id.clone())
            } else {
                None
            },
            fullscreen_art: listening && self.win.is_fullscreen(),
        };
        let prior = self.rendered_page.borrow();
        // A live video sink keeps its own presentation clock during navigation.
        let live_video =
            page == Page::Video || prior.as_ref().is_some_and(|p| p.page == Page::Video);
        let animate = !live_video && prior.as_ref().is_some_and(|p| p != &view);
        drop(prior);
        self.rendered_page.replace(Some(view));
        self.motion.prepare(
            animate,
            !matches!(page, Page::Collection(_)),
            &self.thumb,
            self.playback_artwork().as_deref(),
            self.thumb_art.borrow().as_deref(),
        );
        if live_video {
            self.motion.cancel();
        }
        clear(&self.body);
        self.browser.take();
        self.videos.take();
        self.lyrics.borrow_mut().clear();
        self.lyric_scroll.borrow_mut().take();
        self.active_line.set(None);
        self.follow.set(true);
        self.update_surface(&page);
        self.update_thumbnail();
        let mut child = self.nav.first_child();
        while let Some(w) = child {
            if let Ok(b) = w.clone().downcast::<gtk::Button>() {
                let selected = match b.label().as_deref() {
                    Some("device") => page == Page::Devices,
                    Some("quickplay") => page == Page::Quick,
                    Some("now playing") => {
                        matches!(
                            page,
                            Page::Now | Page::Wall | Page::Lyrics | Page::Showcase | Page::Video
                        )
                    }
                    Some("collection") => matches!(
                        page,
                        Page::Collection(_) | Page::Album(_) | Page::Artist(_) | Page::Playlist(_)
                    ),
                    _ => false,
                };
                if selected {
                    b.add_css_class("selected")
                } else {
                    b.remove_css_class("selected")
                }
            }
            child = w.next_sibling();
        }
        if page == Page::Video {
            self.embedded_view();
            return;
        }
        if matches!(page, Page::Now | Page::Showcase | Page::Wall) {
            self.hero(page == Page::Showcase, page == Page::Wall);
            return;
        }
        if let Page::Collection(tab) = &page {
            self.collection_page(tab);
            return;
        }
        if page == Page::Quick {
            self.quickplay_page();
            return;
        }
        let panel = col(10);
        panel.add_css_class("content");
        self.body.append(&panel);
        let title = match &page {
            Page::Quick => "quickplay",
            Page::Queue => "up next",
            Page::Lyrics => "lyrics",
            Page::Playlist(n) | Page::Artist(n) => n,
            Page::Mix(_) => "mixview",
            Page::Wall => "collection art wall",
            Page::Devices => "devices",
            Page::Album(_) => "album",
            _ => "",
        };
        let head = row(12);
        panel.append(&head);
        head.append(&label(title, "heading"));
        head.append(&gap());
        let list = col(4);
        let scroll = scroller(&list);
        panel.append(&scroll);
        match page {
            Page::Queue => {
                let q = self.queue.borrow().clone();
                head.append(&app_button(self, "Save playlist", |u| u.save_playlist()));
                if q.is_empty() {
                    list.append(&label("Play an album to start a queue.", "subtitle"));
                }
                self.track_rows(&list, q, true);
            }
            Page::Album(key) => {
                let tracks: Vec<_> = self
                    .db
                    .borrow()
                    .tracks
                    .iter()
                    .filter(|t| album_key(t) == key)
                    .cloned()
                    .collect();
                if let Some(t) = tracks.first().cloned() {
                    list.append(&label(&t.album, "section"));
                    let artist = t.album_artist.clone();
                    list.append(&app_button(self, &artist.clone(), move |u| {
                        u.go(Page::Artist(artist.clone()))
                    }));
                    let actions = row(4);
                    list.append(&actions);
                    let group = tracks.clone();
                    actions.append(&app_button(self, "Play album", move |u| {
                        u.start(group.clone(), 0, None)
                    }));
                    let pin_track = t.clone();
                    let pinned = self.album_is_pinned(&t);
                    actions.append(&app_button(
                        self,
                        if pinned { "Unpin" } else { "Pin to Quickplay" },
                        move |u| {
                            u.set_album_pinned(&pin_track, !pinned);
                            u.persist();
                            u.render()
                        },
                    ));
                    let tt = t.clone();
                    actions.append(&app_button(self, "Choose cover", move |u| {
                        u.choose_cover(tt.clone())
                    }));
                    let tt = t.clone();
                    actions.append(&app_button(self, "Smart DJ", move |u| u.mix(&tt)));
                    actions.append(&app_button(self, "Mixview", move |u| {
                        u.go(Page::Mix(key.clone()))
                    }));
                    self.track_rows(&list, tracks, false);
                }
            }
            Page::Artist(name) => {
                let tracks: Vec<_> = self
                    .db
                    .borrow()
                    .tracks
                    .iter()
                    .filter(|t| t.artist == name || t.album_artist == name)
                    .cloned()
                    .collect();
                if let Some(t) = tracks.iter().find(|t| t.artist == name).or(tracks.first()) {
                    let d = if t.artist == name {
                        self.cached(t)
                    } else {
                        enrichment::Details::default()
                    };
                    let links = row(8);
                    for (s, url) in [("Official website", d.website), ("Bandcamp", d.bandcamp)] {
                        if let Some(url) = url {
                            links.append(&gtk::LinkButton::with_label(&url, s));
                        }
                    }
                    list.append(&links);
                    list.append(&label("Albums in your collection", "section"));
                    self.album_grid(&list, &tracks);
                    let seed = t.clone();
                    head.append(&app_button(self, "Smart DJ", move |u| u.mix(&seed)));
                    let albums: BTreeSet<_> = tracks.iter().map(album_key).collect();
                    let related: BTreeSet<_> = self
                        .db
                        .borrow()
                        .tracks
                        .iter()
                        .filter(|x| albums.contains(&album_key(x)) && x.artist != name)
                        .map(|x| x.artist.clone())
                        .collect();
                    if !related.is_empty() {
                        list.append(&label("Shared releases", "section"));
                        for a in related {
                            list.append(&app_button(self, &a.clone(), move |u| {
                                u.go(Page::Artist(a.clone()))
                            }));
                        }
                    }
                }
            }
            Page::Mix(key) => {
                let db = self.db.borrow();
                let seed = db.tracks.iter().find(|t| album_key(t) == key);
                if let Some(seed) = seed {
                    list.append(&label(
                        &format!("{} · {}", seed.album_artist, seed.album),
                        "section",
                    ));
                    let credits: BTreeSet<_> = db
                        .tracks
                        .iter()
                        .filter(|x| album_key(x) == key)
                        .flat_map(|x| [x.artist.clone(), x.album_artist.clone()])
                        .collect();
                    let mut seen = BTreeSet::new();
                    for t in &db.tracks {
                        let k = album_key(t);
                        if k == key || seen.contains(&k) {
                            continue;
                        }
                        let reason =
                            if credits.contains(&t.artist) || credits.contains(&t.album_artist) {
                                "Shared artist"
                            } else if t.genre == seed.genre && seed.genre != "Unknown genre" {
                                &seed.genre
                            } else {
                                continue;
                            };
                        seen.insert(k.clone());
                        let r = row(4);
                        r.append(&app_button(
                            self,
                            &format!("{} · {}   /   {}", t.album_artist, t.album, reason),
                            move |u| u.go(Page::Mix(k.clone())),
                        ));
                        let k = album_key(t);
                        r.append(&app_button(self, "View album", move |u| {
                            u.go(Page::Album(k.clone()))
                        }));
                        list.append(&r);
                    }
                    if seen.is_empty() {
                        list.append(&label(
                            "No related albums in this collection yet.",
                            "subtitle",
                        ));
                    }
                }
            }
            Page::Playlist(name) => {
                let db = self.db.borrow();
                let tracks = if let Some(rule) = db.rules.get(&name) {
                    db.tracks
                        .iter()
                        .filter(|t| state::matches(t, rule))
                        .cloned()
                        .collect()
                } else {
                    db.playlists
                        .get(&name)
                        .into_iter()
                        .flatten()
                        .filter_map(|p| self.track(p))
                        .collect()
                };
                drop(db);
                self.track_rows(&list, tracks, false);
            }
            Page::Lyrics => self.lyric_view(&list, &scroll, &head),
            Page::Devices => self.devices(&list),
            _ => {}
        }
    }
    fn hero(self: &Rc<Self>, showcase: bool, wall: bool) {
        let track = self.current().or_else(|| {
            self.db
                .borrow()
                .current
                .as_ref()
                .and_then(|p| self.track(p))
        });
        let art = track.as_ref().and_then(|t| self.cover(t));
        let overlay = gtk::Overlay::new();
        overlay.set_vexpand(true);
        overlay.set_overflow(gtk::Overflow::Hidden);
        overlay.set_child(Some(&col(0)));
        if showcase && let Some(t) = &track {
            showcase::compose(self, &overlay, t);
        }
        if wall {
            let grid = gtk::Grid::new();
            grid.set_row_homogeneous(true);
            grid.set_column_homogeneous(true);
            let mut seen = BTreeSet::new();
            let art: Vec<_> = self
                .db
                .borrow()
                .tracks
                .iter()
                .filter_map(|t| self.cover(t))
                .filter(|p| seen.insert(p.clone()))
                .take(48)
                .collect();
            if !art.is_empty() {
                for i in 0..48 {
                    let p = visual::picture(Some(&art[i as usize % art.len()]), 64);
                    p.set_hexpand(true);
                    p.set_vexpand(true);
                    grid.attach(&p, i % 8, i / 8, 1, 1);
                }
                grid.set_opacity(0.38);
                overlay.add_overlay(&grid);
            }
        }
        let content = if !showcase && self.compact.get() && self.win.height() > 620 {
            col(12)
        } else {
            row(34)
        };
        content.add_css_class("hero");
        content.set_halign(gtk::Align::Center);
        content.set_valign(gtk::Align::Center);
        let size = if showcase && track.is_some() {
            content.set_halign(gtk::Align::Start);
            content.set_valign(gtk::Align::End);
            content.set_margin_start(24);
            content.set_margin_bottom(24);
            content.add_css_class("showcase-identity");
            content.set_spacing(12);
            if self.compact.get() { 65 } else { 106 }
        } else if self.compact.get() {
            175
        } else {
            295
        };
        let sleeve = visual::picture(art.as_deref(), size);
        content.append(&sleeve);
        let info = col(9);
        info.set_valign(gtk::Align::Center);
        content.append(&info);
        if !showcase {
            info.append(&label(
                if wall {
                    "COLLECTION ART WALL"
                } else {
                    "NOW PLAYING"
                },
                "eyebrow",
            ));
        }
        let title = if showcase {
            track.as_ref().map(|t| t.artist.as_str())
        } else {
            track.as_ref().map(|t| t.title.as_str())
        };
        let h = label(
            title.unwrap_or("Your music collection"),
            if showcase {
                "showcase-artist"
            } else {
                "hero-title"
            },
        );
        h.set_max_width_chars(22);
        h.set_wrap(true);
        h.set_lines(2);
        info.append(&h);
        if let Some(t) = track {
            if showcase {
                info.append(&label(&t.album, "showcase-album"));
                info.append(&label(&t.title, "showcase-title"));
            } else {
                info.append(&label(&t.artist, "hero-artist"));
                info.append(&label(&t.album, "hero-album"));
            }
            let buttons = row(5);
            let artist = t.artist.clone();
            buttons.append(&app_button(self, "Artist", move |u| {
                u.go(Page::Artist(artist.clone()))
            }));
            buttons.append(&app_button(self, "Lyrics", |u| u.go(Page::Lyrics)));
            buttons.append(&app_button(self, "Up next", |u| u.go(Page::Queue)));
            if showcase {
                let a = t.artist.clone();
                buttons.append(&app_button(self, "Choose artist photo", move |u| {
                    let v = u.clone();
                    let artist = a.clone();
                    gtk::FileDialog::builder()
                        .title("Choose artist photo")
                        .build()
                        .open(Some(&u.win), None::<&gio::Cancellable>, move |r| {
                            if let Ok(file) = r
                                && let Some(path) = file.path()
                                && visual::palette(&path).is_some()
                            {
                                match gnune::library::save_image(&path, &data_dir().join("artwork"))
                                {
                                    Ok(file) => {
                                        v.db.borrow_mut().portraits.insert(artist, file);
                                        v.update_cover();
                                    }
                                    Err(e) => {
                                        v.notice(&e.to_string());
                                        return;
                                    }
                                }
                                v.persist();
                                v.render();
                            }
                        });
                }));
                let d = self.cached(&t);
                if let Some(site) = d.website {
                    buttons.append(&gtk::LinkButton::with_label(&site, "Website"));
                }
            }
            info.append(&buttons);
        } else {
            info.append(&label(
                "Open a track or add a folder to start listening.",
                "subtitle",
            ));
            let actions = row(10);
            actions.set_margin_top(12);
            let b = app_button(self, "Add music folder", |u| u.choose(true));
            b.add_css_class("primary");
            actions.append(&b);
            let b = app_button(self, "Open file", |u| u.choose(false));
            b.add_css_class("secondary");
            actions.append(&b);
            info.append(&actions);
        }
        if self.win.is_fullscreen()
            && !wall
            && !showcase
            && let Some(path) = art.as_ref()
        {
            let p = gtk::Picture::new();
            image_loader::load(
                &p,
                path,
                self.win.width().max(self.win.height()) * self.win.scale_factor(),
            );
            p.set_content_fit(gtk::ContentFit::Contain);
            p.set_can_shrink(true);
            p.set_hexpand(true);
            p.set_vexpand(true);
            self.motion.set_art_target(&p, art.as_deref());
            overlay.add_overlay(&p);
        } else {
            self.motion.set_art_target(&sleeve, art.as_deref());
            overlay.add_overlay(&content);
        }
        let click = gtk::GestureClick::new();
        let u = self.clone();
        click.connect_pressed(move |_, n, _, _| {
            if n == 2 {
                u.fullscreen();
            }
        });
        overlay.add_controller(click);
        self.body.append(&overlay);
    }
    fn song_list(self: &Rc<Self>, list: &gtk::Box, tracks: Vec<Track>) {
        let dest = gtk::FlowBox::new();
        dest.set_selection_mode(gtk::SelectionMode::None);
        dest.set_min_children_per_line(1);
        for name in self.db.borrow().playlists.keys() {
            let n = name.clone();
            let b = app_button(self, name, move |u| u.go(Page::Playlist(n.clone())));
            b.set_tooltip_text(Some("Drop a song here to add it to this playlist"));
            self.drop_destination(&b, name.clone());
            dest.insert(&b, -1);
        }
        list.append(&dest);
        self.track_rows(list, tracks, false);
    }
    fn album_grid(self: &Rc<Self>, list: &gtk::Box, tracks: &[Track]) {
        let mut seen = BTreeSet::new();
        let albums = tracks
            .iter()
            .filter(|t| seen.insert(album_key(t)))
            .cloned()
            .collect::<Vec<_>>();
        self.album_batch(list, Rc::new(albums), 0);
    }
    fn album_batch(self: &Rc<Self>, list: &gtk::Box, albums: Rc<Vec<Track>>, offset: usize) {
        let grid = gtk::FlowBox::new();
        grid.set_homogeneous(true);
        grid.set_selection_mode(gtk::SelectionMode::None);
        grid.set_valign(gtk::Align::Start);
        grid.set_column_spacing(6);
        grid.set_max_children_per_line(8);
        grid.set_min_children_per_line(1);
        for t in albums.iter().skip(offset).take(48) {
            let k = album_key(t);
            let tile = col(4);
            tile.append(&visual::picture(self.cover(t).as_deref(), 126));
            let l = label(&t.album, "track");
            l.set_max_width_chars(17);
            tile.append(&l);
            let l = label(&t.album_artist, "small");
            l.set_max_width_chars(18);
            tile.append(&l);
            let b = app_button(self, "", move |u| u.go(Page::Album(k.clone())));
            b.add_css_class("tile");
            b.set_child(Some(&tile));
            grid.insert(&b, -1);
        }
        list.append(&grid);
        if offset + 48 < albums.len() {
            let more = gtk::Button::with_label("Show more albums");
            let u = self.clone();
            let container = list.downgrade();
            more.connect_clicked(move |button| {
                if let Some(container) = container.upgrade() {
                    container.remove(button);
                    u.album_batch(&container, albums.clone(), offset + 48);
                }
            });
            list.append(&more);
        }
    }
    fn track_rows(self: &Rc<Self>, list: &gtk::Box, tracks: Vec<Track>, edit: bool) {
        self.track_batch(list, Rc::new(tracks), edit, 0);
    }
    fn track_batch(
        self: &Rc<Self>,
        list: &gtk::Box,
        tracks: Rc<Vec<Track>>,
        edit: bool,
        offset: usize,
    ) {
        const BATCH: usize = 80;
        for (i, t) in tracks.iter().enumerate().skip(offset).take(BATCH) {
            let r = row(8);
            r.add_css_class("item");
            let n = label(
                &format!("{:02}", t.track_number.unwrap_or(i as u32 + 1)),
                "small",
            );
            n.set_width_chars(3);
            r.append(&n);
            let names = col(2);
            names.append(&label(&t.title, "track"));
            names.append(&label(&format!("{} · {}", t.artist, t.album), "small"));
            let all = tracks.clone();
            let b = app_button(self, "", move |u| u.start(all.as_ref().clone(), i, None));
            b.set_child(Some(&names));
            b.set_hexpand(true);
            r.append(&b);
            let drag = gtk::DragSource::new();
            drag.set_actions(gtk::gdk::DragAction::COPY);
            let id = t.id.clone();
            drag.connect_prepare(move |_, _, _| {
                Some(gtk::gdk::ContentProvider::for_value(&id.to_value()))
            });
            b.add_controller(drag);
            r.append(&label(&time(t.duration), "clock"));
            if edit {
                for (s, d) in [("↑", -1), ("↓", 1), ("×", 0)] {
                    let b = app_button(self, s, move |u| u.edit_queue(i, d));
                    b.set_tooltip_text(Some(match d {
                        -1 => "Move earlier",
                        1 => "Move later",
                        _ => "Remove from queue",
                    }));
                    r.append(&b);
                }
            } else {
                let tt = t.clone();
                let m = gtk::MenuButton::builder()
                    .icon_name("view-more-symbolic")
                    .tooltip_text("Track actions")
                    .build();
                let pop = gtk::Popover::new();
                let actions = col(2);
                let tt2 = t.clone();
                actions.append(&app_button(self, "Add to queue", move |u| {
                    u.append_track(tt2.clone())
                }));
                actions.append(&app_button(self, "Add to playlist…", move |u| {
                    let track = tt.clone();
                    u.prompt("Playlist name", "Add track", move |u, n| {
                        u.db.borrow_mut()
                            .playlists
                            .entry(n)
                            .or_default()
                            .push(track.path.clone());
                        u.persist();
                    });
                }));
                pop.set_child(Some(&actions));
                m.set_popover(Some(&pop));
                r.append(&m);
            }
            list.append(&r);
        }
        if offset + BATCH < tracks.len() {
            let more = gtk::Button::with_label("Show more tracks");
            let u = self.clone();
            let container = list.downgrade();
            more.connect_clicked(move |button| {
                if let Some(container) = container.upgrade() {
                    container.remove(button);
                    u.track_batch(&container, tracks.clone(), edit, offset + BATCH);
                }
            });
            list.append(&more);
        }
    }
    fn mix(self: &Rc<Self>, seed: &Track) {
        let mut q: Vec<_> = self
            .db
            .borrow()
            .tracks
            .iter()
            .filter(|x| {
                x.id != seed.id
                    && (x.artist == seed.artist
                        || (x.genre == seed.genre && x.genre != "Unknown genre"))
            })
            .cloned()
            .collect();
        q.shuffle(&mut rand::rng());
        q.truncate(39);
        q.insert(0, seed.clone());
        self.start(q, 0, None);
        self.go(Page::Queue);
    }
    fn edit_queue(self: &Rc<Self>, i: usize, delta: i32) {
        let mut q = self.queue.borrow().clone();
        if i >= q.len() {
            return;
        }
        let to = if delta == 0 {
            None
        } else {
            let j = i as i32 + delta;
            if !(0..q.len() as i32).contains(&j) {
                return;
            }
            Some(j as usize)
        };
        if !self.player.snapshot().finished
            && let Err(e) = self.player.edit(i, to)
        {
            self.notice(&e);
            return;
        }
        let entry = q.remove(i);
        if let Some(to) = to {
            q.insert(to, entry);
        }
        if q.is_empty() {
            self.command(self.player.set_queue(&[], 0));
            self.current.borrow_mut().take();
            self.db.borrow_mut().current = None;
            if self.media.borrow().is_none() {
                self.mini_title.set_text("Nothing playing");
            }
            self.update_cover();
        }
        self.remember_queue(q);
        self.render();
    }
    fn remember_queue(&self, q: Vec<Track>) {
        self.db.borrow_mut().recent_queue = q.iter().map(|t| t.path.clone()).collect();
        *self.sequence.borrow_mut() = q.clone();
        *self.queue.borrow_mut() = q;
        self.persist();
    }
    fn append_track(self: &Rc<Self>, t: Track) {
        let mut q = self.queue.borrow().clone();
        if q.is_empty() {
            self.start(vec![t], 0, None);
            return;
        }
        if !self.player.snapshot().finished
            && let Err(e) = self.player.append(&t)
        {
            self.notice(&e);
            return;
        }
        q.push(t);
        self.remember_queue(q);
    }
    fn save_playlist(self: &Rc<Self>) {
        self.prompt("Playlist name", "Save queue", |u, n| {
            let paths = u.queue.borrow().iter().map(|t| t.path.clone()).collect();
            if u.db.borrow().playlists.contains_key(&n) || u.db.borrow().rules.contains_key(&n) {
                u.notice("A playlist already has that name. Choose a new name.");
                return;
            }
            u.db.borrow_mut().playlists.insert(n.clone(), paths);
            u.persist();
            u.go(Page::Playlist(n));
        });
    }
    fn playlists(self: &Rc<Self>, list: &gtk::Box) {
        list.append(&app_button(self, "New playlist", |u| {
            u.prompt("Playlist name", "Create", |u, n| {
                u.db.borrow_mut().playlists.entry(n).or_default();
                u.persist();
                u.render();
            })
        }));
        list.append(&app_button(self, "New rule-based playlist", |u| {
            u.prompt(
                "Match artist, album, genre, or title",
                "Save rule",
                |u, q| {
                    u.db.borrow_mut().rules.insert(format!("Matches · {q}"), q);
                    u.persist();
                    u.render();
                },
            )
        }));
        let names: Vec<_> = self
            .db
            .borrow()
            .playlists
            .keys()
            .chain(self.db.borrow().rules.keys())
            .cloned()
            .collect();
        for n in names {
            let name = n.clone();
            let b = app_button(self, &n, move |u| u.go(Page::Playlist(name.clone())));
            self.drop_destination(&b, n);
            list.append(&b);
        }
    }
    fn drop_destination(self: &Rc<Self>, b: &gtk::Button, n: String) {
        if self.db.borrow().playlists.contains_key(&n) {
            let target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::COPY);
            let u = self.clone();
            target.connect_drop(move |_, v, _, _| {
                if let Ok(id) = v.get::<String>()
                    && let Some(t) = u.db.borrow().tracks.iter().find(|x| x.id == id).cloned()
                {
                    glib::idle_add_local_once({
                        let u = u.clone();
                        let n = n.clone();
                        move || {
                            u.db.borrow_mut()
                                .playlists
                                .entry(n)
                                .or_default()
                                .push(t.path);
                            u.persist();
                        }
                    });
                    return true;
                }
                false
            });
            b.add_controller(target);
        }
    }
    fn lyric_view(self: &Rc<Self>, list: &gtk::Box, scroll: &gtk::ScrolledWindow, head: &gtk::Box) {
        let Some(t) = self.current() else {
            list.append(&label("Play a track to see its lyrics.", "subtitle"));
            return;
        };
        head.append(&app_button(self, "Follow current line", |u| {
            u.follow.set(true);
            u.active_line.set(None);
        }));
        let text = t
            .lyrics
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .or_else(|| self.cached(&t).lyrics);
        let Some(text) = text else {
            list.append(&label(
                "No lyrics available. Add an .lrc file beside this track.",
                "subtitle",
            ));
            return;
        };
        list.append(&label(&format!("{} · {}", t.artist, t.title), "subtitle"));
        for line in state::lines(&text) {
            if let Some(at) = line.at {
                let b = app_button(self, &line.text, move |u| {
                    u.command(u.seek_playback(at));
                    u.follow.set(true);
                });
                b.add_css_class("lyric");
                b.set_halign(gtk::Align::Start);
                self.lyrics.borrow_mut().push((at, b.clone()));
                list.append(&b);
            } else {
                let l = label(&line.text, "lyric");
                l.set_wrap(true);
                list.append(&l);
            }
        }
        *self.lyric_scroll.borrow_mut() = Some(scroll.clone());
        let ev = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let u = self.clone();
        ev.set_propagation_phase(gtk::PropagationPhase::Capture);
        ev.connect_scroll(move |_, _, _| {
            u.follow.set(false);
            glib::Propagation::Proceed
        });
        scroll.add_controller(ev);
    }
    pub(super) fn highlight_lyric(&self, pos: f64) {
        let lines = self.lyrics.borrow();
        let active = lines.partition_point(|(t, _)| *t <= pos).checked_sub(1);
        if active == self.active_line.get() {
            return;
        }
        if let Some(i) = self.active_line.replace(active)
            && let Some((_, b)) = lines.get(i)
        {
            b.remove_css_class("current-line");
        }
        if let Some(i) = active {
            let b = &lines[i].1;
            b.add_css_class("current-line");
            if self.follow.get()
                && let Some(s) = self.lyric_scroll.borrow().as_ref()
            {
                let a = s.vadjustment();
                a.set_value(
                    (b.compute_bounds(&b.parent().unwrap())
                        .map_or(0., |r| f64::from(r.y()))
                        - a.page_size() * 0.4)
                        .max(0.),
                );
            }
        }
    }
    pub(super) fn share_queue(self: &Rc<Self>) {
        if self.server.borrow().is_some() {
            self.go(Page::Devices);
            return;
        }
        let q = self.queue.borrow().clone();
        if q.is_empty() {
            self.notice("Play a playlist or album to select music to share.");
            return;
        }
        let tx = self.tx.clone();
        match sync::start(q, move |p, t| {
            let _ = tx.send(Event::Progress(p, t));
        }) {
            Ok(s) => {
                *self.server.borrow_mut() = Some(s);
                self.notice("");
                self.go(Page::Devices)
            }
            Err(e) => self.notice(&e),
        }
    }
    pub(super) fn copy_pairing_link(&self) {
        if let Some(s) = self.server.borrow().as_ref() {
            self.win.clipboard().set_text(&s.url);
            self.notice("Pairing link copied.");
        }
    }
    fn devices(self: &Rc<Self>, list: &gtk::Box) {
        list.append(&label(
            "Offline playlists for iPhone and Android",
            "section",
        ));
        list.append(&label(
            "Share the current queue over your local HTTPS connection.",
            "subtitle",
        ));
        if let Some(s) = self.server.borrow().as_ref() {
            let l = label(&s.url, "track");
            l.set_selectable(true);
            l.set_ellipsize(gtk::pango::EllipsizeMode::None);
            l.set_wrap(true);
            list.append(&l);
            let copy = gtk::Button::with_label("Copy pairing link");
            copy.set_action_name(Some("app.copy-pairing-link"));
            list.append(&copy);
            list.append(&app_button(self, "Stop sharing", |u| {
                if let Some(session) = u.server.borrow_mut().take() {
                    std::thread::spawn(move || drop(session));
                }
                u.render();
            }));
        } else {
            let pair = gtk::Button::with_label("Pair phone with current queue");
            pair.set_action_name(Some("app.share-queue"));
            list.append(&pair);
        }
        let progress = self.db.borrow().phone_progress.clone();
        for (p, n) in progress {
            if let Some(t) = self.track(&p) {
                let b = gtk::Button::with_label(&format!("Resume {} at {}", t.title, time(n)));
                b.set_action_name(Some("app.resume-from-phone"));
                b.set_action_target_value(Some(&t.id.to_variant()));
                list.append(&b);
            }
        }
    }
}
fn scroller(child: &gtk::Box) -> gtk::ScrolledWindow {
    let s = gtk::ScrolledWindow::new();
    s.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    s.set_vexpand(true);
    s.set_child(Some(child));
    s
}
