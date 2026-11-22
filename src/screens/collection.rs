use super::*;

const ALBUM_BATCH: usize = 48;

impl Ui {
    pub(super) fn collection_page(self: &Rc<Self>, tab: &str) {
        let panel = col(12);
        panel.add_css_class("collection");
        panel.set_vexpand(true);
        self.body.append(&panel);
        let videos = tab == "videos" || tab.starts_with("videos/");
        let mode = if videos {
            tab.strip_prefix("videos/").unwrap_or("artists")
        } else {
            tab
        };
        let mode = if mode.starts_with("playlist/") {
            "playlists"
        } else {
            mode
        };
        let music = !videos && !["photos", "podcasts"].contains(&tab);
        let categories = row(8);
        categories.add_css_class("collection-categories");
        panel.append(&categories);
        for (name, destination) in [
            ("MUSIC", "artists"),
            ("VIDEOS", "videos"),
            ("PICTURES", "photos"),
            ("PODCASTS", "podcasts"),
        ] {
            let b = app_button(self, name, move |u| {
                u.go(Page::Collection(destination.into()))
            });
            if (music && destination == "artists")
                || (videos && destination == "videos")
                || tab == destination
            {
                b.add_css_class("selected");
            }
            categories.append(&b);
        }
        categories.append(&gap());
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("SEARCH"));
        search.add_css_class("collection-search");
        search.set_width_chars(16);
        search.set_max_width_chars(20);
        search.set_text(&self.filters.borrow());
        search.set_visible((music || videos) && mode != "playlists");
        categories.append(&search);
        if music || videos {
            let modes = row(8);
            modes.add_css_class("collection-modes");
            modes.set_halign(gtk::Align::End);
            panel.append(&modes);
            for destination in ["artists", "genres", "albums", "songs", "playlists"] {
                let b = app_button(self, &destination.to_uppercase(), move |u| {
                    u.go(Page::Collection(if videos && destination == "artists" {
                        "videos".into()
                    } else if videos {
                        format!("videos/{destination}")
                    } else {
                        destination.into()
                    }))
                });
                if destination == mode {
                    b.add_css_class("selected");
                }
                modes.append(&b);
            }
        }
        let results = col(0);
        results.set_vexpand(true);
        panel.append(&results);
        if videos {
            search.set_placeholder_text(Some("SEARCH VIDEOS"));
            self.video_collection(&results, tab);
            let weak = results.downgrade();
            let ui = self.clone();
            let tab = tab.to_string();
            search.connect_search_changed(move |entry| {
                *ui.filters.borrow_mut() = entry.text().into();
                if let Some(results) = weak.upgrade() {
                    clear(&results);
                    ui.video_collection(&results, &tab);
                }
            });
        } else if music && tab != "playlists" {
            let weak = results.downgrade();
            let u = self.clone();
            let mode = tab.to_string();
            search.connect_search_changed(move |entry| {
                *u.filters.borrow_mut() = entry.text().to_string();
                if let Some(results) = weak.upgrade() {
                    clear(&results);
                    u.collection_music(&results, &mode);
                }
            });
            self.collection_music(&results, tab);
        } else if tab == "photos" {
            self.photos(&results);
        } else {
            let list = col(4);
            results.append(&scroller(&list));
            match tab {
                "playlists" => self.playlists(&list),
                "podcasts" => self.podcasts(&list),
                _ => {}
            }
        }
    }

    fn collection_music(self: &Rc<Self>, results: &gtk::Box, tab: &str) {
        let tracks = self
            .db
            .borrow()
            .tracks
            .iter()
            .filter(|t| state::matches(t, &self.filters.borrow()))
            .cloned()
            .collect();
        self.collection_tracks(results, tab, tracks, false, None);
    }

    pub(super) fn collection_tracks(
        self: &Rc<Self>,
        results: &gtk::Box,
        tab: &str,
        tracks: Vec<Track>,
        videos: bool,
        previous: Option<(Option<String>, Option<String>)>,
    ) {
        self.browser.take();
        if tracks.is_empty() {
            let empty = col(10);
            empty.add_css_class("collection-empty");
            empty.set_halign(gtk::Align::Start);
            empty.set_margin_top(24);
            let library_empty = if videos {
                self.db
                    .borrow()
                    .assets
                    .iter()
                    .all(|p| media::is_photo_path(p))
            } else {
                self.db.borrow().tracks.is_empty()
            };
            if library_empty {
                empty.append(&label(
                    if videos {
                        "your video collection"
                    } else {
                        "your music collection"
                    },
                    "heading",
                ));
                let add = app_button(
                    self,
                    if videos {
                        "Add video folder"
                    } else {
                        "Add music folder"
                    },
                    |u| u.choose(true),
                );
                add.add_css_class("primary");
                add.set_halign(gtk::Align::Start);
                empty.append(&add);
            } else {
                empty.append(&label(
                    if videos {
                        "No matching videos."
                    } else {
                        "No matching music."
                    },
                    "subtitle",
                ));
            }
            results.append(&empty);
            return;
        }
        if tab == "songs" {
            results.append(&label(&count(tracks.len(), "SONG"), "collection-count"));
            if videos {
                self.video_song_grid(results, &tracks, None);
            } else {
                let list = col(4);
                results.append(&scroller(&list));
                self.song_list(&list, tracks);
            }
            return;
        }
        let genre = tab == "genres";
        let names = group_names(&tracks, genre);
        let all_label = if genre { "All genres" } else { "All artists" };
        let options: Vec<_> = std::iter::once(all_label.to_string())
            .chain(names)
            .collect();
        let dropdown =
            gtk::DropDown::from_strings(&options.iter().map(String::as_str).collect::<Vec<_>>());
        dropdown.add_css_class("collection-group-picker");
        dropdown.set_tooltip_text(Some(if genre {
            "Filter by genre"
        } else {
            "Filter by artist"
        }));
        dropdown.set_visible(false);
        results.append(&dropdown);

        let panes = row(18);
        panes.add_css_class("collection-panes");
        panes.set_vexpand(true);
        results.append(&panes);
        let groups = pane("collection-artists");
        groups.set_size_request(176, -1);
        groups.set_visible(tab != "albums");
        panes.append(&groups);
        groups.append(&count_heading(
            options.len() - 1,
            if genre { "GENRE" } else { "ARTIST" },
            "A–Z",
        ));

        let albums = pane("collection-albums");
        albums.set_hexpand(true);
        albums.set_size_request(240, -1);
        panes.append(&albums);
        let songs = pane("collection-songs");
        songs.set_size_request(230, -1);
        songs.set_hexpand(true);
        panes.append(&songs);
        let (selected_group, selected_album) = previous.unwrap_or_default();
        let selected_album = selected_album.filter(|key| {
            tracks.iter().any(|t| {
                in_group(t, selected_group.as_deref(), genre)
                    && (if videos {
                        super::videos::video_album_key(t)
                    } else {
                        album_key(t)
                    }) == *key
            })
        });
        let selected_group = selected_group.filter(|name| options[1..].contains(name));
        let selected_index = selected_group
            .as_ref()
            .and_then(|name| options.iter().position(|n| n == name))
            .unwrap_or(0);
        let browser = Rc::new(Browser {
            videos,
            ui: self.clone(),
            tracks: Rc::new(tracks),
            genre,
            albums_only: tab == "albums",
            panes: panes.downgrade(),
            groups: groups.downgrade(),
            picker: dropdown.downgrade(),
            visible_songs: Default::default(),
            album_reset: Default::default(),
            group: RefCell::new(selected_group),
            album: RefCell::new(selected_album),
            albums: albums.downgrade(),
            songs: songs.downgrade(),
        });
        self.browser.replace(Rc::downgrade(&browser));
        let model = gtk::StringList::new(&options.iter().map(String::as_str).collect::<Vec<_>>());
        let selection = gtk::SingleSelection::new(Some(model));
        selection.set_selected(selected_index as u32);
        dropdown.set_selected(selected_index as u32);
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let name = label("", "collection-artist-name");
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            item.set_child(Some(&name));
        });
        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let name = item.child().and_downcast::<gtk::Label>().unwrap();
            let text = item
                .item()
                .and_downcast::<gtk::StringObject>()
                .unwrap()
                .string();
            name.set_text(&text);
            name.set_tooltip_text(Some(&text));
        });
        let view = gtk::ListView::new(Some(selection.clone()), Some(factory));
        view.add_css_class("collection-artist-list");
        groups.append(&scroll_widget(&view));
        let state = browser.clone();
        let picker = dropdown.downgrade();
        selection.connect_selected_notify(move |selection| {
            let index = selection.selected();
            *state.group.borrow_mut() = if index == 0 {
                None
            } else {
                selection
                    .selected_item()
                    .and_downcast::<gtk::StringObject>()
                    .map(|s| s.string().to_string())
            };
            state.album.borrow_mut().take();
            if let Some(picker) = picker.upgrade()
                && picker.selected() != index
            {
                picker.set_selected(index);
            }
            state.render_albums();
            state.render_songs();
        });
        let weak_selection = selection.downgrade();
        dropdown.connect_selected_notify(move |picker| {
            if let Some(selection) = weak_selection.upgrade()
                && selection.selected() != picker.selected()
            {
                selection.set_selected(picker.selected());
            }
        });
        browser.render_albums();
        browser.render_songs();

        browser.set_compact(self.compact.get());
    }
}

struct SongSelection {
    tracks: Rc<Vec<Track>>,
    model: gtk::SingleSelection,
}
pub(crate) struct Browser {
    ui: Rc<Ui>,
    videos: bool,
    tracks: Rc<Vec<Track>>,
    genre: bool,
    albums_only: bool,
    panes: glib::WeakRef<gtk::Box>,
    groups: glib::WeakRef<gtk::Box>,
    picker: glib::WeakRef<gtk::DropDown>,
    visible_songs: RefCell<Option<SongSelection>>,
    album_reset: glib::WeakRef<gtk::Button>,
    group: RefCell<Option<String>>,
    album: RefCell<Option<String>>,
    albums: glib::WeakRef<gtk::Box>,
    songs: glib::WeakRef<gtk::Box>,
}

impl Browser {
    pub(crate) fn set_compact(&self, compact: bool) {
        if let Some(panes) = self.panes.upgrade() {
            panes.set_orientation(if compact {
                gtk::Orientation::Vertical
            } else {
                gtk::Orientation::Horizontal
            });
        }
        if let Some(groups) = self.groups.upgrade() {
            groups.set_visible(!compact && !self.albums_only);
        }
        if let Some(picker) = self.picker.upgrade() {
            picker.set_visible(compact && !self.albums_only);
        }
        if let Some(albums) = self.albums.upgrade() {
            albums.set_vexpand(!compact);
            albums.set_size_request(
                if compact { -1 } else { 240 },
                if compact { 190 } else { -1 },
            );
        }
    }
    pub(crate) fn highlight(&self, track: Option<&Track>) {
        if let Some(songs) = self.visible_songs.borrow().as_ref() {
            let index = track.and_then(|t| songs.tracks.iter().position(|row| row.path == t.path));
            songs
                .model
                .set_selected(index.map_or(gtk::INVALID_LIST_POSITION, |i| i as u32));
        }
    }
    pub(super) fn selection(&self) -> (Option<String>, Option<String>) {
        (self.group.borrow().clone(), self.album.borrow().clone())
    }
    fn key(&self, t: &Track) -> String {
        if self.videos {
            super::videos::video_album_key(t)
        } else {
            album_key(t)
        }
    }
    fn filtered(&self, album: bool) -> Vec<Track> {
        self.tracks
            .iter()
            .filter(|t| in_group(t, self.group.borrow().as_deref(), self.genre))
            .filter(|t| {
                !album
                    || self
                        .album
                        .borrow()
                        .as_ref()
                        .is_none_or(|key| self.key(t) == *key)
            })
            .cloned()
            .collect()
    }

    fn album_title<'a>(&self, track: &'a Track) -> &'a str {
        if self.videos && track.album == "Unknown album" {
            &track.title
        } else {
            &track.album
        }
    }

    fn render_albums(self: &Rc<Self>) {
        let Some(pane) = self.albums.upgrade() else {
            return;
        };
        clear(&pane);
        let tracks = self.filtered(false);
        let mut seen = BTreeSet::new();
        let mut albums: Vec<_> = tracks
            .into_iter()
            .filter(|t| seen.insert(self.key(t)))
            .collect();
        albums.sort_by_cached_key(|t| {
            (
                self.album_title(t).to_lowercase(),
                t.album_artist.to_lowercase(),
                t.path.clone(),
            )
        });
        let heading = count_heading(albums.len(), "ALBUM", "A–Z");
        heading.append(&gap());
        let reset = gtk::Button::with_label("ALL");
        reset.add_css_class("collection-reset");
        reset.set_tooltip_text(Some("Show songs from all albums"));
        let state = self.clone();
        reset.connect_clicked(move |_| {
            state.album.borrow_mut().take();
            state.render_albums();
            state.render_songs();
        });
        reset.set_visible(self.album.borrow().is_some());
        self.album_reset.set(Some(&reset));
        heading.append(&reset);
        pane.append(&heading);
        let list = col(8);
        pane.append(&scroller(&list));
        self.album_batch(&list, Rc::new(albums), 0);
    }

    fn album_batch(self: &Rc<Self>, list: &gtk::Box, albums: Rc<Vec<Track>>, offset: usize) {
        let grid = gtk::FlowBox::new();
        grid.set_selection_mode(gtk::SelectionMode::None);
        grid.set_min_children_per_line(1);
        grid.set_max_children_per_line(6);
        grid.set_column_spacing(8);
        grid.set_row_spacing(12);
        grid.set_valign(gtk::Align::Start);
        for t in albums.iter().skip(offset).take(ALBUM_BATCH) {
            let tile = col(2);
            tile.set_halign(gtk::Align::Start);
            let sleeve = gtk::Overlay::new();
            sleeve.set_size_request(76, 76);
            sleeve.set_halign(gtk::Align::Start);
            if self.videos {
                sleeve.add_overlay(&self.ui.video_poster(&t.path, 76, 76));
            } else {
                sleeve.add_overlay(&visual::picture(self.ui.cover(t).as_deref(), 76));
            }
            tile.append(&sleeve);
            for (text, class) in [
                (self.album_title(t), "collection-album-title"),
                (t.album_artist.as_str(), "collection-album-artist"),
            ] {
                let name = label(text, class);
                name.set_width_chars(12);
                name.set_max_width_chars(12);
                name.set_ellipsize(gtk::pango::EllipsizeMode::End);
                tile.append(&name);
            }
            let key = self.key(t);
            let b = gtk::Button::new();
            b.add_css_class("collection-album");
            if self.album.borrow().as_ref() == Some(&key) {
                b.add_css_class("selected");
            }
            b.set_child(Some(&tile));
            b.set_tooltip_text(Some(&format!(
                "{} — {}",
                t.album_artist,
                self.album_title(t)
            )));
            let state = self.clone();
            b.connect_clicked(move |button| {
                *state.album.borrow_mut() = Some(key.clone());
                if let Some(pane) = state.albums.upgrade() {
                    clear_album_highlights(&pane);
                }
                if let Some(reset) = state.album_reset.upgrade() {
                    reset.set_visible(true);
                }
                button.add_css_class("selected");
                state.render_songs();
            });
            grid.insert(&b, -1);
        }
        list.append(&grid);
        if offset + ALBUM_BATCH < albums.len() {
            let more = gtk::Button::with_label("MORE ALBUMS");
            let state = self.clone();
            let weak = list.downgrade();
            more.connect_clicked(move |button| {
                if let Some(list) = weak.upgrade() {
                    list.remove(button);
                    state.album_batch(&list, albums.clone(), offset + ALBUM_BATCH);
                }
            });
            list.append(&more);
        }
    }

    fn render_songs(self: &Rc<Self>) {
        let Some(pane) = self.songs.upgrade() else {
            return;
        };
        clear(&pane);
        let mut tracks = self.filtered(true);
        if self.album.borrow().is_some() {
            tracks.sort_by_cached_key(|t| {
                (
                    t.disc_number.unwrap_or(1),
                    t.track_number.unwrap_or(0),
                    t.title.to_lowercase(),
                )
            });
        } else {
            tracks.sort_by_cached_key(|t| t.title.to_lowercase());
        }
        let by_album = self.album.borrow().is_some();
        let heading = count_heading(
            tracks.len(),
            "SONG",
            if by_album { "BY ALBUM" } else { "A–Z" },
        );
        heading.append(&gap());
        if !self.videos
            && let Some(key) = self.album.borrow().clone()
        {
            let details = app_button(&self.ui, "ALBUM DETAILS", move |u| {
                u.go(Page::Album(key.clone()))
            });
            details.add_css_class("collection-details");
            heading.append(&details);
        }
        pane.append(&heading);
        if by_album && let Some(t) = tracks.first() {
            pane.append(&label(self.album_title(t), "collection-release"));
        }
        let tracks = Rc::new(tracks);
        let model =
            gtk::StringList::new(&tracks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>());
        let selection = gtk::SingleSelection::new(Some(model));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);
        selection.set_selected(gtk::INVALID_LIST_POSITION);
        let factory = gtk::SignalListItemFactory::new();
        let bound_tracks = tracks.clone();
        let ui = self.ui.clone();
        let videos = self.videos;
        factory.connect_bind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(t) = bound_tracks.get(item.position() as usize) {
                if videos {
                    item.set_child(Some(&ui.video_song_row(t)));
                } else {
                    let title = label("", "collection-song-title");
                    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    title.set_text(&if by_album && let Some(number) = t.track_number {
                        format!("{number:02}   {}", t.title)
                    } else {
                        t.title.clone()
                    });
                    title.set_tooltip_text(Some(&format!(
                        "{} · {} · {}",
                        t.artist,
                        t.album,
                        time(t.duration)
                    )));
                    item.set_child(Some(&title));
                }
            }
        });
        factory.connect_unbind(|_, item| {
            item.downcast_ref::<gtk::ListItem>()
                .unwrap()
                .set_child(gtk::Widget::NONE);
        });
        let view = gtk::ListView::new(Some(selection.clone()), Some(factory));
        self.visible_songs.replace(Some(SongSelection {
            tracks: tracks.clone(),
            model: selection,
        }));
        self.highlight(self.ui.current().as_ref());
        view.add_css_class("collection-song-list");
        view.set_single_click_activate(true);
        let u = self.ui.clone();
        let videos = self.videos;
        view.connect_activate(move |_, position| {
            if videos {
                if let Some(t) = tracks.get(position as usize) {
                    u.show_asset(&t.path);
                }
            } else {
                u.start(tracks.as_ref().clone(), position as usize, None);
            }
        });
        pane.append(&scroll_widget(&view));
    }
}

fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "S" })
}

fn count_heading(n: usize, noun: &str, order: &str) -> gtk::Box {
    let heading = row(4);
    heading.append(&label(&count(n, noun), "collection-count"));
    heading.append(&label(order, "collection-order"));
    heading
}

fn pane(class: &str) -> gtk::Box {
    let pane = col(8);
    pane.add_css_class("collection-pane");
    pane.add_css_class(class);
    pane.set_vexpand(true);
    pane
}

fn clear_album_highlights(widget: &impl IsA<gtk::Widget>) {
    let widget = widget.as_ref();
    if widget.has_css_class("collection-album") {
        widget.remove_css_class("selected");
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        clear_album_highlights(&current);
        child = current.next_sibling();
    }
}

fn scroll_widget(widget: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    scroll.set_child(Some(widget));
    scroll
}

fn in_group(track: &Track, name: Option<&str>, genre: bool) -> bool {
    name.is_none_or(|name| {
        if genre {
            track.genre == name
        } else {
            track.artist == name || track.album_artist == name
        }
    })
}

fn group_names(tracks: &[Track], genre: bool) -> Vec<String> {
    let mut names = BTreeSet::new();
    for t in tracks {
        if genre {
            names.insert(t.genre.clone());
        } else {
            names.insert(t.artist.clone());
            names.insert(t.album_artist.clone());
        }
    }
    let mut names: Vec<_> = names.into_iter().collect();
    names.sort_by_cached_key(|name| name.to_lowercase());
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artist_groups_include_track_and_album_credits_and_can_reset() {
        let t = Track {
            artist: "Guest".into(),
            album_artist: "Band".into(),
            genre: "Rock".into(),
            ..Track::default()
        };
        assert_eq!(
            group_names(std::slice::from_ref(&t), false),
            ["Band", "Guest"]
        );
        assert!(in_group(&t, Some("Band"), false));
        assert!(in_group(&t, Some("Guest"), false));
        assert!(!in_group(&t, Some("Other"), false));
        assert!(in_group(&t, None, false));
        assert!(in_group(&t, Some("Rock"), true));
        assert!(!in_group(&t, Some("Band"), true));
    }
}
