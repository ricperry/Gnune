use super::*;

struct Video {
    path: PathBuf,
    title: String,
    credit: String,
    duration: f64,
}

impl Video {
    fn read(ui: &Ui, path: &Path) -> Self {
        let title = ui.video_title(path);
        let db = ui.db.borrow();
        let tags = db.video_tags.get(path);
        Self {
            path: path.into(),
            title,
            credit: tags.map(|t| t.song().credit()).unwrap_or_default(),
            duration: tags.map_or(0., |t| t.duration),
        }
    }
}

struct PosterTarget {
    picture: glib::WeakRef<gtk::Picture>,
    size: i32,
}

/// Refresh only the collection content; embedded playback and navigation stay intact.
#[derive(Clone)]
pub(crate) struct VideoGallery {
    results: glib::WeakRef<gtk::Box>,
    tab: String,
    metadata_pending: Rc<Cell<bool>>,
    posters: Rc<RefCell<HashMap<PathBuf, Vec<PosterTarget>>>>,
}
impl VideoGallery {
    pub(crate) fn refresh_poster(&self, path: &Path) {
        let file = media::poster_path(path, &data_dir().join("posters"));
        if let Some(pictures) = self.posters.borrow_mut().get_mut(path) {
            let available = file.is_file();
            pictures.retain(|target| {
                if let Some(picture) = target.picture.upgrade() {
                    if available {
                        image_loader::replace_when_ready(
                            &picture,
                            &file,
                            target.size * picture.scale_factor(),
                        );
                    } else {
                        image_loader::cancel(&picture);
                        picture.set_paintable(None::<&gtk::gdk::Texture>);
                    }
                    true
                } else {
                    false
                }
            });
        }
    }
    pub(crate) fn refresh_metadata(&self, ui: &Rc<Ui>) {
        if self.metadata_pending.replace(true) {
            return;
        }
        let gallery = self.clone();
        let ui = ui.clone();
        glib::idle_add_local_once(move || {
            gallery.metadata_pending.set(false);
            gallery.rebuild_metadata(&ui);
        });
    }
    fn rebuild_metadata(&self, ui: &Rc<Ui>) {
        if let Some(results) = self.results.upgrade() {
            let previous = ui.browser.borrow().upgrade().map(|b| b.selection());
            let scrolls = scroll_positions(&results);
            let focused =
                gtk::prelude::GtkWindowExt::focus(&ui.win).map(|w| w.widget_name().to_string());
            clear(&results);
            ui.video_collection_with_selection(&results, &self.tab, previous);
            let weak = results.downgrade();
            glib::timeout_add_local_once(Duration::from_millis(40), move || {
                if let Some(results) = weak.upgrade() {
                    let widgets = scroll_widgets(&results);
                    for (widget, (h, v)) in widgets.iter().zip(scrolls) {
                        widget.hadjustment().set_value(h);
                        widget.vadjustment().set_value(v);
                    }
                    if let Some(name) = focused.filter(|n| n.starts_with("video-")) {
                        restore_focus(&results, &name);
                    }
                }
            });
        }
    }
}

fn known(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.into()
    } else {
        value.trim().into()
    }
}

fn video_track(path: &Path, title: String, tags: Option<&recognition::VideoTags>) -> Track {
    let song = tags.map(|t| t.song()).cloned().unwrap_or_default();
    let artist = known(&song.artist, "Unknown artist");
    let album_artist = tags
        .filter(|t| {
            t.saved.as_ref().is_none_or(|saved| {
                saved.artist == t.embedded.artist && saved.album == t.embedded.album
            })
        })
        .and_then(|t| t.album_artist.as_deref())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| artist.clone());
    Track {
        id: path.to_string_lossy().into(),
        path: path.into(),
        title,
        artist,
        album_artist,
        album: known(&song.album, "Unknown album"),
        genre: known(tags.map(|t| t.genre()).unwrap_or(""), "Unknown genre"),
        duration: tags.map_or(0., |t| t.duration),
        track_number: tags.and_then(|t| t.track_number),
        disc_number: tags.and_then(|t| t.disc_number),
        ..Track::default()
    }
}

pub(super) fn video_album_key(t: &Track) -> String {
    if t.album == "Unknown album" {
        format!("video/{}", t.path.display())
    } else {
        serde_json::to_string(&(&t.album_artist, &t.album)).unwrap()
    }
}

impl Ui {
    pub(super) fn video_collection(self: &Rc<Self>, results: &gtk::Box, tab: &str) {
        self.video_collection_with_selection(results, tab, None);
    }
    fn video_collection_with_selection(
        self: &Rc<Self>,
        results: &gtk::Box,
        tab: &str,
        previous: Option<(Option<String>, Option<String>)>,
    ) {
        self.videos.replace(Some(VideoGallery {
            results: results.downgrade(),
            tab: tab.into(),
            metadata_pending: Default::default(),
            posters: Default::default(),
        }));
        let mode = tab.strip_prefix("videos/").unwrap_or("artists");
        if mode == "playlists" {
            self.video_playlists(results);
            return;
        }
        if let Some(name) = mode.strip_prefix("playlist/") {
            self.video_playlist(results, name);
            return;
        }
        let mut tracks: Vec<_> = self
            .db
            .borrow()
            .assets
            .iter()
            .filter(|p| !media::is_photo_path(p))
            .map(|path| {
                video_track(
                    path,
                    self.video_title(path),
                    self.db.borrow().video_tags.get(path),
                )
            })
            .filter(|t| video_matches(t, &self.filters.borrow()))
            .collect();
        tracks.sort_by(|a, b| {
            a.title
                .to_lowercase()
                .cmp(&b.title.to_lowercase())
                .then_with(|| a.artist.to_lowercase().cmp(&b.artist.to_lowercase()))
                .then_with(|| a.path.cmp(&b.path))
        });
        self.collection_tracks(results, mode, tracks, true, previous);
    }

    pub(super) fn video_song_grid(
        self: &Rc<Self>,
        list: &gtk::Box,
        tracks: &[Track],
        playlist: Option<&str>,
    ) {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        for track in tracks {
            store.append(&glib::BoxedAnyObject::new(track.path.clone()));
        }
        let selection = gtk::NoSelection::new(Some(store));
        let factory = gtk::SignalListItemFactory::new();
        let ui = self.clone();
        let playlist = playlist.map(|name| {
            let paths: Rc<[PathBuf]> = tracks.iter().map(|t| t.path.clone()).collect();
            (name.to_owned(), paths)
        });
        factory.connect_bind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(obj) = item.item() {
                let path = obj
                    .downcast_ref::<glib::BoxedAnyObject>()
                    .unwrap()
                    .borrow::<PathBuf>();
                let card = ui.video_card(
                    &Video::read(&ui, &path),
                    playlist
                        .as_ref()
                        .map(|(_, paths)| (paths.clone(), item.position() as usize)),
                );
                if let Some((name, _)) = &playlist {
                    let name = name.clone();
                    let path = path.clone();
                    card.append(&app_button(&ui, "Remove from playlist", move |u| {
                        if let Some(paths) = u.db.borrow_mut().video_playlists.get_mut(&name) {
                            paths.retain(|p| p != &path);
                        }
                        u.persist();
                        u.render();
                    }));
                }
                item.set_child(Some(&card));
            }
        });
        factory.connect_unbind(|_, item| {
            item.downcast_ref::<gtk::ListItem>()
                .unwrap()
                .set_child(gtk::Widget::NONE)
        });
        let grid = gtk::GridView::new(Some(selection), Some(factory));
        grid.add_css_class("video-grid");
        grid.set_min_columns(1);
        grid.set_max_columns(32);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&grid));
        list.append(&scroll);
    }

    pub(super) fn video_song_row(self: &Rc<Self>, t: &Track) -> gtk::Box {
        let r = row(6);
        r.append(&self.video_poster(&t.path, 48, 27));
        let title = label(&t.title, "collection-song-title");
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_max_width_chars(1);
        title.set_hexpand(true);
        title.set_tooltip_text(Some(&format!(
            "{} · {} · {}",
            t.artist,
            t.album,
            time(t.duration)
        )));
        r.append(&title);
        self.video_actions(&r, &t.path, &t.title);
        r
    }

    fn video_actions(self: &Rc<Self>, target: &gtk::Box, path: &Path, title: &str) {
        let id = enrichment::key(&[&path.to_string_lossy()]);
        let info = icon("help-about-symbolic", "Song info");
        info.update_property(&[gtk::accessible::Property::Label(&format!(
            "Song info for {title}"
        ))]);
        let p = path.to_owned();
        action(self, &info, move |u| u.video_info(&p));
        info.set_widget_name(&format!("video-info-{id}"));
        info.add_css_class("video-info-link");
        target.append(&info);
        let add = gtk::MenuButton::new();
        add.set_icon_name("list-add-symbolic");
        add.set_tooltip_text(Some("Add to video playlist"));
        add.update_property(&[gtk::accessible::Property::Label(&format!(
            "Add {title} to video playlist"
        ))]);
        add.set_widget_name(&format!("video-add-{id}"));
        add.add_css_class("video-info-link");
        let popover = gtk::Popover::new();
        let ui = self.clone();
        let path = path.to_owned();
        popover.connect_show(move |popover| ui.video_playlist_choices(popover, &path));
        add.set_popover(Some(&popover));
        target.append(&add);
    }

    fn video_playlist_choices(self: &Rc<Self>, popover: &gtk::Popover, path: &Path) {
        let choices = col(4);
        let p = path.to_owned();
        let menu = popover.downgrade();
        choices.append(&app_button(self, "New video playlist…", move |u| {
            if let Some(menu) = menu.upgrade() {
                menu.popdown();
            }
            let p = p.clone();
            u.prompt("Video playlist name", "Create and add", move |u, name| {
                u.db.borrow_mut().add_video_to_playlist(name, p.clone());
                u.persist();
            });
        }));
        for (name, members) in &self.db.borrow().video_playlists {
            let p = path.to_owned();
            let dest = name.clone();
            let menu = popover.downgrade();
            let pick = app_button(self, name, move |u| {
                u.db.borrow_mut()
                    .add_video_to_playlist(dest.clone(), p.clone());
                u.persist();
                if let Some(menu) = menu.upgrade() {
                    menu.popdown();
                }
            });
            pick.set_sensitive(!members.iter().any(|p| p == path));
            choices.append(&pick);
        }
        popover.set_child(Some(&choices));
    }

    fn video_playlists(self: &Rc<Self>, results: &gtk::Box) {
        self.browser.take();
        results.append(&app_button(self, "New video playlist", |u| {
            u.prompt("Video playlist name", "Create", |u, name| {
                u.db.borrow_mut()
                    .video_playlists
                    .entry(name.clone())
                    .or_default();
                u.persist();
                u.go(Page::Collection(format!("videos/playlist/{name}")));
            });
        }));
        let list = col(8);
        results.append(&scroller(&list));
        for (name, paths) in self.db.borrow().video_playlists.clone() {
            let n = name.clone();
            let r = row(8);
            r.append(&app_button(self, &name, move |u| {
                u.go(Page::Collection(format!("videos/playlist/{n}")))
            }));
            r.append(&label(&count_video(paths.len()), "small"));
            let n = name.clone();
            r.append(&app_button(self, "Remove playlist", move |u| {
                u.db.borrow_mut().video_playlists.remove(&n);
                u.persist();
                u.render();
            }));
            list.append(&r);
        }
        if self.db.borrow().video_playlists.is_empty() {
            list.append(&label(
                "Create a playlist, then use + beside a video to add it.",
                "subtitle",
            ));
        }
    }

    fn video_playlist(self: &Rc<Self>, results: &gtk::Box, name: &str) {
        self.browser.take();
        results.append(&label(name, "collection-release"));
        results.append(&app_button(self, "Add videos from Songs", |u| {
            u.go(Page::Collection("videos/songs".into()))
        }));
        let paths = self
            .db
            .borrow()
            .video_playlists
            .get(name)
            .cloned()
            .unwrap_or_default();
        if paths.is_empty() {
            results.append(&label(
                "This playlist is empty. Use + beside a video to add it.",
                "subtitle",
            ));
        }
        let tracks: Vec<_> = paths
            .iter()
            .map(|p| video_track(p, self.video_title(p), self.db.borrow().video_tags.get(p)))
            .collect();
        self.video_song_grid(results, &tracks, Some(name));
    }
    pub(super) fn video_poster(&self, path: &Path, width: i32, height: i32) -> gtk::Overlay {
        let poster = gtk::Overlay::new();
        poster.add_css_class("video-poster");
        poster.set_size_request(width, height);
        let placeholder = gtk::Image::from_icon_name("video-x-generic-symbolic");
        placeholder.set_pixel_size(24);
        poster.set_child(Some(&placeholder));
        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_can_shrink(true);
        let file = media::poster_path(path, &data_dir().join("posters"));
        if file.is_file() {
            image_loader::load(&picture, &file, width.max(height) * self.win.scale_factor());
        }
        poster.add_overlay(&picture);
        if let Some(gallery) = self.videos.borrow().as_ref() {
            let mut posters = gallery.posters.borrow_mut();
            let pictures = posters.entry(path.into()).or_default();
            pictures.retain(|target| target.picture.upgrade().is_some());
            pictures.push(PosterTarget {
                picture: picture.downgrade(),
                size: width.max(height),
            });
        }
        poster
    }

    fn video_card(
        self: &Rc<Self>,
        video: &Video,
        playlist: Option<(Rc<[PathBuf]>, usize)>,
    ) -> gtk::Box {
        let tile = col(3);
        tile.add_css_class("video-tile");
        tile.set_halign(gtk::Align::Start);
        let path = video.path.clone();
        let play = app_button(self, "", move |ui| {
            if let Some((paths, index)) = &playlist {
                ui.play_video_playlist(paths.clone(), *index);
            } else {
                ui.show_asset(&path);
            }
        });
        play.set_widget_name(&format!(
            "video-play-{}",
            enrichment::key(&[&video.path.to_string_lossy()])
        ));
        play.add_css_class("video-play");
        play.update_property(&[gtk::accessible::Property::Label(&format!(
            "Play {}",
            video.title
        ))]);
        let aspect = gtk::AspectFrame::new(0., 0.5, 16. / 9., false);
        aspect.set_size_request(188, 106);
        let poster = self.video_poster(&video.path, 188, 106);
        if video.duration >= 1. {
            let duration = label(&time(video.duration), "video-duration");
            duration.set_halign(gtk::Align::End);
            duration.set_valign(gtk::Align::End);
            poster.add_overlay(&duration);
        }
        aspect.set_child(Some(&poster));
        play.set_child(Some(&aspect));
        tile.append(&play);
        let caption = row(2);
        let names = col(1);
        names.set_hexpand(true);
        let title = label(&video.title, "track");
        title.set_max_width_chars(1);
        title.set_tooltip_text(Some(&video.title));
        names.append(&title);
        if !video.credit.is_empty() {
            let credit = label(&video.credit, "small");
            credit.set_max_width_chars(1);
            credit.set_tooltip_text(Some(&video.credit));
            names.append(&credit);
        }
        caption.append(&names);
        self.video_actions(&caption, &video.path, &video.title);
        tile.append(&caption);
        tile
    }
}

fn count_video(n: usize) -> String {
    format!("{n} video{}", if n == 1 { "" } else { "s" })
}

fn scroll_widgets(widget: &impl IsA<gtk::Widget>) -> Vec<gtk::ScrolledWindow> {
    let widget = widget.as_ref();
    let mut found = Vec::new();
    if let Some(scroll) = widget.downcast_ref::<gtk::ScrolledWindow>() {
        found.push(scroll.clone());
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        found.extend(scroll_widgets(&current));
        child = current.next_sibling();
    }
    found
}
fn scroll_positions(widget: &impl IsA<gtk::Widget>) -> Vec<(f64, f64)> {
    scroll_widgets(widget)
        .iter()
        .map(|s| (s.hadjustment().value(), s.vadjustment().value()))
        .collect()
}
fn restore_focus(widget: &impl IsA<gtk::Widget>, name: &str) -> bool {
    let widget = widget.as_ref();
    if widget.widget_name() == name {
        return widget.grab_focus();
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        if restore_focus(&current, name) {
            return true;
        }
        child = current.next_sibling();
    }
    false
}

fn video_matches(t: &Track, query: &str) -> bool {
    let key = format!(
        "{} {} {} {} {} {}",
        t.path.file_name().unwrap_or_default().to_string_lossy(),
        t.title,
        t.artist,
        t.album_artist,
        t.album,
        t.genre
    )
    .to_lowercase();
    query
        .split_whitespace()
        .all(|q| key.contains(&q.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use recognition::{Song, VideoTags};

    #[test]
    fn unknown_videos_keep_individual_album_posters_and_filename_search() {
        let a = video_track(Path::new("/films/a.mkv"), "A".into(), None);
        let b = video_track(Path::new("/films/b.mkv"), "B".into(), None);
        assert_eq!(a.artist, "Unknown artist");
        assert_eq!(a.genre, "Unknown genre");
        assert_ne!(video_album_key(&a), video_album_key(&b));
        assert!(video_matches(&a, "A.MKV unknown"));
        assert!(!video_matches(&a, "B.MKV"));
    }

    #[test]
    fn video_albums_use_credits_and_saved_edits_preserve_native_grouping_when_possible() {
        let mut tags = VideoTags {
            embedded: Song {
                title: "Original".into(),
                artist: "Guest".into(),
                album: "Compilation".into(),
            },
            album_artist: Some("Various artists".into()),
            genre: Some("Rock".into()),
            track_number: Some(3),
            disc_number: Some(2),
            ..Default::default()
        };
        let first = video_track(Path::new("one.mp4"), "Original".into(), Some(&tags));
        tags.saved = Some(Song {
            title: "Corrected".into(),
            ..tags.embedded.clone()
        });
        let corrected = video_track(Path::new("two.mp4"), "Corrected".into(), Some(&tags));
        assert_eq!(video_album_key(&first), video_album_key(&corrected));
        assert_eq!(corrected.album_artist, "Various artists");
        assert_eq!(corrected.track_number, Some(3));
        assert_eq!(corrected.disc_number, Some(2));
        assert!(video_matches(&corrected, "rock corrected"));
        tags.saved.as_mut().unwrap().artist = "Reviewed artist".into();
        let edited = video_track(Path::new("two.mp4"), "Corrected".into(), Some(&tags));
        assert_eq!(edited.artist, "Reviewed artist");
        assert_ne!(video_album_key(&first), video_album_key(&edited));
    }
}
