use super::*;
use std::collections::BTreeSet;
mod bluetooth;
mod collection;
mod pairing;
mod quickplay;
mod videos;
pub(crate) use collection::Browser;
pub(crate) use quickplay::SmartDjRotation;
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
        let listening = matches!(
            page,
            Page::Now | Page::Wall | Page::Showcase | Page::PodcastNow
        );
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
        if prior.as_ref() == Some(&view)
            && match &page {
                Page::Collection(tab) if tab == "podcasts/browse" => self.podcast_browser_mounted(),
                Page::Collection(tab) if tab == "podcasts" => self
                    .podcast_grid
                    .borrow()
                    .as_ref()
                    .is_some_and(|view| view.root.parent().is_some()),
                Page::Podcast(feed) => self
                    .podcast_show
                    .borrow()
                    .as_ref()
                    .is_some_and(|view| &view.feed == feed && view.root.parent().is_some()),
                _ => false,
            }
        {
            // Sharing retries, fullscreen notifications and playback updates are
            // independent of discovery. Keep its scroller, focus and rows mounted.
            drop(prior);
            self.update_surface(&page);
            self.update_thumbnail();
            return;
        }

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
        self.podcast_browser.borrow_mut().view.take();
        self.podcast_grid.borrow_mut().take();
        self.podcast_show.borrow_mut().take();
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
                    Some("quickplay") => page == Page::Quick,
                    Some("now playing") => {
                        matches!(
                            page,
                            Page::Now
                                | Page::Wall
                                | Page::Lyrics
                                | Page::Showcase
                                | Page::Video
                                | Page::PodcastNow
                        )
                    }
                    Some("collection") => matches!(
                        page,
                        Page::Collection(_)
                            | Page::Album(_)
                            | Page::Artist(_)
                            | Page::Playlist(_)
                            | Page::Podcast(_)
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
        if page == Page::PodcastNow {
            self.podcast_listening_page();
            return;
        }
        if page == Page::Video {
            self.embedded_view();
            return;
        }
        if matches!(page, Page::Now | Page::Showcase | Page::Wall) {
            self.hero(page == Page::Showcase, page == Page::Wall);
            return;
        }
        if let Page::Podcast(feed) = &page {
            let root = col(12);
            root.set_vexpand(true);
            root.add_css_class("collection");
            self.body.append(&root);
            self.podcast_show_page(feed, &root);
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
                let mut tracks: Vec<_> = self
                    .db
                    .borrow()
                    .tracks
                    .iter()
                    .filter(|t| album_key(t) == key)
                    .cloned()
                    .collect();
                sort_album_tracks(&mut tracks);
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
    pub(super) fn hero(self: &Rc<Self>, showcase: bool, wall: bool) {
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
        let grid = gtk::FlowBox::new();
        grid.set_homogeneous(true);
        grid.set_selection_mode(gtk::SelectionMode::None);
        grid.set_valign(gtk::Align::Start);
        grid.set_column_spacing(6);
        grid.set_max_children_per_line(8);
        grid.set_min_children_per_line(1);
        list.append(&grid);
        self.album_tiles(&grid, &albums[..albums.len().min(48)]);
        if albums.len() <= 48 {
            return;
        }
        let scroll = list
            .ancestor(gtk::ScrolledWindow::static_type())
            .and_downcast::<gtk::ScrolledWindow>()
            .unwrap();
        let albums = Rc::new(albums);
        let loaded = Rc::new(Cell::new(48));
        let pending = Rc::new(Cell::new(false));
        let ui = Rc::downgrade(self);
        let weak_grid = grid.downgrade();
        let weak_scroll = scroll.downgrade();
        // Keep the artist's links and shared releases in the same scrolling page.
        // Append to one grid so resizing never leaves seams between batches.
        let reveal = Rc::new(move || {
            if loaded.get() == albums.len() || pending.replace(true) {
                return;
            }
            let pending = pending.clone();
            let ui = ui.clone();
            let weak_grid = weak_grid.clone();
            let weak_scroll = weak_scroll.clone();
            let loaded = loaded.clone();
            let albums = albums.clone();
            glib::idle_add_local_once(move || {
                pending.set(false);
                let (Some(ui), Some(grid), Some(scroll)) =
                    (ui.upgrade(), weak_grid.upgrade(), weak_scroll.upgrade())
                else {
                    return;
                };
                if !grid.is_mapped() {
                    return;
                }
                // Use the grid's end, not the whole page: a long footer must
                // never prevent the next albums from appearing.
                if grid.compute_bounds(&scroll).is_some_and(|bounds| {
                    bounds.y() + bounds.height() <= scroll.height() as f32 * 2.0
                }) {
                    let end = (loaded.get() + 48).min(albums.len());
                    ui.album_tiles(&grid, &albums[loaded.get()..end]);
                    loaded.set(end);
                }
            });
        });
        let on_scroll = reveal.clone();
        scroll
            .vadjustment()
            .connect_value_changed(move |_| on_scroll());
        let on_resize = reveal.clone();
        scroll.vadjustment().connect_changed(move |_| on_resize());
        grid.connect_map(move |_| reveal());
    }
    fn album_tiles(self: &Rc<Self>, grid: &gtk::FlowBox, albums: &[Track]) {
        for t in albums {
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
            let ui = self.clone();
            let track = t.clone();
            tile_context_menu(&b, move |tile| ui.quickplay_menu(tile, &track));
            grid.insert(&b, -1);
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
    fn sharing_started(self: &Rc<Self>, session: sync::Session) {
        session.publish(&self.db.borrow());
        *self.server.borrow_mut() = Some(session);
        self.sharing_pending.set(false);
        self.sharing_retry.set(None);
        self.sharing_retry_delay.set(5);
        self.sharing_problem.borrow_mut().take();
        self.notice("");
        self.render();
    }
    fn sharing_failed(self: &Rc<Self>, error: sync::SharingError, background: bool) {
        let changed = self.sharing_problem.borrow().as_ref() != Some(&error.message);
        self.sharing_pending
            .set(error.waiting || self.sharing_pending.get());
        self.sharing_retry.set(if error.retryable {
            let delay = self.sharing_retry_delay.get();
            self.sharing_retry_delay.set((delay * 2).min(30));
            Some(Instant::now() + Duration::from_secs(delay))
        } else {
            None
        });
        *self.sharing_problem.borrow_mut() = Some(error.message.clone());
        if !background {
            self.notice(&error.message);
        }
        if changed && *self.page.borrow() == Page::Devices {
            self.render();
        }
    }
    pub(super) fn restore_sharing(self: &Rc<Self>) {
        if self.protect || self.server.borrow().is_some() {
            return;
        }
        let tx = self.tx.clone();
        let result = sync::resume(&self.db.borrow(), move |change| {
            let _ = tx.send(Event::Phone(change));
        });
        match result {
            Ok(Some(session)) => self.sharing_started(session),
            Ok(None) => {
                self.sharing_pending.set(false);
                self.sharing_retry.set(None);
                self.sharing_problem.borrow_mut().take();
                self.render();
            }
            Err(error) => self.sharing_failed(error, true),
        }
    }
    fn stop_sharing(self: &Rc<Self>) {
        self.sharing_retry.set(None);
        self.sharing_pending.set(false);
        self.sharing_retry_delay.set(5);
        self.sharing_problem.borrow_mut().take();
        let stopped = sync::stop_remembered();
        if let Some(session) = self.server.borrow_mut().take() {
            std::thread::spawn(move || drop(session));
        }
        match stopped {
            Ok(()) => self.notice("Sharing stopped. It will stay off when Gnune reopens."),
            Err(error) => self.notice(&format!("Sharing stopped for this session, but the saved setting could not be disabled: {error}. It may resume when Gnune reopens.")),
        }
        self.render();
    }
    pub(super) fn show_devices(self: &Rc<Self>) {
        self.navigate(Page::Devices);
    }
    pub(super) fn copy_pairing_link(&self) {
        if let Some(s) = self.server.borrow().as_ref() {
            if s.can_pair_phone() && s.pairing_status() == sync::PairingStatus::Ready {
                self.win.clipboard().set_text(&s.url);
                self.notice("Pairing link copied.");
            } else {
                self.notice("Choose Pair phone before copying a link.");
            }
        }
    }
    pub(super) fn pair_phone(self: &Rc<Self>) {
        if let Err(error) = sync::pairing_setup() {
            self.notice(&error);
            self.navigate(Page::Devices);
            return;
        }
        let updated = self
            .server
            .borrow_mut()
            .as_mut()
            .map(|s| s.pair_phone(&self.db.borrow()));
        if let Some(result) = updated {
            match result {
                Ok(()) => {
                    self.notice("");
                    self.show_devices();
                }
                Err(error) => self.notice(&error),
            }
            return;
        }
        let tx = self.tx.clone();
        match sync::pair_phone(&self.db.borrow(), move |change| {
            let _ = tx.send(Event::Phone(change));
        }) {
            Ok(session) => {
                self.sharing_started(session);
                self.show_devices();
            }
            Err(error) => self.sharing_failed(error, false),
        }
    }
    fn devices(self: &Rc<Self>, list: &gtk::Box) {
        list.set_spacing(24);
        list.set_halign(gtk::Align::Start);
        let note = |text: &str, width: i32| {
            let l = label(text, "subtitle");
            l.set_ellipsize(gtk::pango::EllipsizeMode::None);
            l.set_wrap(true);
            l.set_max_width_chars(width);
            l
        };
        let action = |text: &str, name: &str| {
            let button = gtk::Button::with_label(text);
            button.set_action_name(Some(name));
            button.set_halign(gtk::Align::Start);
            button
        };
        let setup = sync::pairing_setup();
        let active = self
            .server
            .borrow()
            .as_ref()
            .is_some_and(|s| s.can_pair_phone());
        if let Some(s) = self.server.borrow().as_ref().filter(|s| s.can_pair_phone()) {
            let status = s.pairing_status();
            let panel = gtk::Box::new(
                if self.compact.get() {
                    gtk::Orientation::Vertical
                } else {
                    gtk::Orientation::Horizontal
                },
                20,
            );
            panel.set_valign(gtk::Align::Start);
            list.append(&panel);
            if status == sync::PairingStatus::Ready {
                match pairing::qr(&s.url) {
                    Ok(qr) => panel.append(&qr),
                    Err(_) => panel.append(&note("QR unavailable. Use Copy pairing link.", 28)),
                }
            }
            let controls = col(8);
            controls.set_valign(gtk::Align::Start);
            controls.set_halign(gtk::Align::Start);
            panel.append(&controls);
            controls.append(&label(
                match status {
                    sync::PairingStatus::Ready => "Scan to pair your phone",
                    sync::PairingStatus::Checking => "Checking Tailscale connection",
                    sync::PairingStatus::Unavailable => "Tailscale connection unavailable",
                    sync::PairingStatus::Used => "Phone paired",
                    sync::PairingStatus::Expired => "Pairing QR expired",
                    sync::PairingStatus::Limited => "Pairing paused",
                },
                "section",
            ));
            let scope = format!("{} songs · videos · podcasts", s.track_count);
            let scope_label = note(&scope, 30);
            scope_label.add_css_class("track");
            controls.append(&scope_label);
            controls.append(&note(match status {
                sync::PairingStatus::Ready => "With Tailscale connected, scan the QR with your phone’s camera. Tap Pair phone on your phone.",
                sync::PairingStatus::Checking => "Your QR will appear when this PC can reach the phone player over Tailscale.",
                sync::PairingStatus::Unavailable => "Reconnect Tailscale on this PC and check that Tailscale Serve is running. Gnune will check again automatically.",
                sync::PairingStatus::Used => "This code has been used. Choose Pair another phone to create a new QR.",
                sync::PairingStatus::Expired => "Codes last 10 minutes. Choose Pair phone to create a new QR.",
                sync::PairingStatus::Limited => "Too many unsuccessful attempts. Choose Pair phone when you are ready to try again.",
            }, 30));
            if status == sync::PairingStatus::Ready {
                controls.append(&note("Single use · valid for 10 minutes.", 30));
            }
            let pair = action(
                match status {
                    sync::PairingStatus::Ready => "Refresh QR",
                    sync::PairingStatus::Used => "Pair another phone",
                    _ => "Pair phone",
                },
                "app.pair-phone",
            );
            pair.set_sensitive(status != sync::PairingStatus::Checking);
            controls.append(&pair);
            if status == sync::PairingStatus::Ready {
                let link = gtk::Expander::new(Some("Pairing link"));
                let details = col(8);
                let url = note(&s.url, 30);
                url.set_selectable(true);
                details.append(&url);
                details.append(&action("Copy pairing link", "app.copy-pairing-link"));
                link.set_child(Some(&details));
                controls.append(&link);
            }
            if s.skipped > 0 {
                list.append(&note(
                    &format!(
                        "{} unavailable files omitted from this snapshot.",
                        s.skipped
                    ),
                    62,
                ));
            }
            // Update a consumed, expired or locked code without creating a new one.
            // The weak panel also ends this timer when another page is mounted.
            let weak_panel = panel.downgrade();
            let weak_ui = Rc::downgrade(self);
            glib::timeout_add_local(Duration::from_secs(1), move || {
                if weak_panel
                    .upgrade()
                    .is_none_or(|panel| panel.root().is_none())
                {
                    return glib::ControlFlow::Break;
                }
                let Some(ui) = weak_ui.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let changed = ui
                    .server
                    .borrow()
                    .as_ref()
                    .is_some_and(|s| s.pairing_status() != status);
                if changed {
                    ui.render();
                    return glib::ControlFlow::Break;
                }
                glib::ControlFlow::Continue
            });
        } else {
            let connection = col(8);
            list.append(&connection);
            connection.append(&label(
                if setup.is_err() {
                    "Connect with Tailscale"
                } else if self.sharing_pending.get() {
                    "Sharing needs attention"
                } else {
                    "Your collection on your phone"
                },
                "section",
            ));
            connection.append(&note(
                if setup.is_err() { "Set up Tailscale on this PC and your phone to pair securely at home or away." }
                else if self.sharing_pending.get() { "The saved connection is unavailable. Pair phone retries with your full collection." }
                else { "Share your music, playlists, collection videos and subscribed podcasts over Tailscale." }, 62,
            ));
            if let Err(problem) = &setup {
                let details = gtk::Expander::new(Some("Tailscale setup"));
                details.set_child(Some(&note(problem, 62)));
                connection.append(&details);
            }
            if let Some(problem) = self.sharing_problem.borrow().as_ref() {
                let details = gtk::Expander::new(Some("Connection details"));
                details.set_child(Some(&note(problem, 62)));
                connection.append(&details);
            }
            let pair = action("Pair phone", "app.pair-phone");
            pair.set_sensitive(setup.is_ok());
            connection.append(&pair);
        }
        let details = col(8);
        details.append(&note("Pair phone refreshes the collection and creates a new single-use QR. Existing phones keep their original permissions; pair them again to grant the full collection. Sharing resumes when Gnune reopens. Stop sharing keeps it off.", 62));
        let permissions = gtk::Expander::new(Some("About sharing"));
        permissions.set_child(Some(&details));
        list.append(&permissions);
        if active || self.server.borrow().is_some() || self.sharing_pending.get() {
            let stop = app_button(self, "Stop sharing", |u| u.stop_sharing());
            stop.set_halign(gtk::Align::Start);
            list.append(&stop);
        }
        let bluetooth = gtk::Expander::new(Some("Bluetooth phones"));
        bluetooth.connect_expanded_notify(|expander| {
            if expander.is_expanded() && expander.child().is_none() {
                expander.set_child(Some(&bluetooth::panel()));
            }
        });
        list.append(&bluetooth);
    }
}
fn scroller(child: &gtk::Box) -> gtk::ScrolledWindow {
    let s = gtk::ScrolledWindow::new();
    s.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    s.set_vexpand(true);
    s.set_child(Some(child));
    s
}

// Capture secondary clicks before GtkWindowHandle can open the window menu.
// Each tile owns one set of controllers; recycled list items replace the tile.
pub(super) fn tile_context_menu<W: IsA<gtk::Widget>>(tile: &W, popup: impl Fn(&W) + 'static) {
    let popup = Rc::new(popup);
    let menu = gtk::GestureClick::new();
    menu.set_button(3);
    menu.set_propagation_phase(gtk::PropagationPhase::Capture);
    let show = popup.clone();
    let weak = tile.downgrade();
    menu.connect_pressed(move |gesture, _, _, _| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        if let Some(tile) = weak.upgrade() {
            tile.grab_focus();
            show(&tile);
        }
    });
    tile.add_controller(menu);
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = tile.downgrade();
    keys.connect_key_pressed(move |_, key, _, mods| {
        if (key == gtk::gdk::Key::Menu
            || (key == gtk::gdk::Key::F10 && mods.contains(gtk::gdk::ModifierType::SHIFT_MASK)))
            && let Some(tile) = weak.upgrade()
        {
            popup(&tile);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    tile.add_controller(keys);
    tile.connect_unmap(|tile| close_tile_menu(tile));
}

pub(super) fn close_tile_menu(tile: &impl IsA<gtk::Widget>) {
    let mut child = tile.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if let Ok(popover) = widget.downcast::<gtk::Popover>() {
            popover.popdown();
            if popover.parent().is_some() {
                popover.unparent();
            }
        }
    }
}
