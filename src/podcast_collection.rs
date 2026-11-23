//! Mounted subscribed-show gallery and episode detail; playback updates keep these widgets alive.
use super::*;

pub(super) struct Gallery {
    pub root: gtk::Box,
    ui: std::rc::Weak<Ui>,
    model: gtk::StringList,
    query: RefCell<String>,
    urls: RefCell<Vec<String>>,
    tiles: Rc<RefCell<HashMap<String, Tile>>>,
    count: gtk::Label,
    scroll: gtk::ScrolledWindow,
    grid: gtk::GridView,
    visibility_pending: Cell<bool>,
}
struct Tile {
    title: gtk::Label,
    publisher: gtk::Label,
    picture: gtk::Picture,
    art_path: RefCell<Option<PathBuf>>,
    root: gtk::Box,
}
pub(super) struct Show {
    pub root: gtk::Box,
    pub feed: String,
    ui: std::rc::Weak<Ui>,
    title: gtk::Label,
    publisher: gtk::Label,
    summary: gtk::Label,
    full_summary: gtk::Label,
    synopsis: gtk::Expander,
    picture: gtk::Picture,
    art_path: RefCell<Option<PathBuf>>,
    message: gtk::Label,
    latest: gtk::Button,
    refresh: gtk::Button,
    subscription: gtk::Button,
    order: gtk::DropDown,
    model: gtk::StringList,
    urls: RefCell<Vec<String>>,
    data: Rc<RefCell<HashMap<String, media::Episode>>>,
    rows: Rc<RefCell<HashMap<String, EpisodeRow>>>,
}
struct EpisodeRow {
    root: gtk::Box,
    title: gtk::Label,
    summary: gtk::Label,
    date: gtk::Label,
}
fn action(ui: &Rc<Ui>, text: &str, callback: impl Fn(&Rc<Ui>) + 'static) -> gtk::Button {
    let weak = Rc::downgrade(ui);
    button(text, move || {
        if let Some(ui) = weak.upgrade() {
            callback(&ui);
        }
    })
}
fn text(value: &str, class: &str) -> gtk::Label {
    let label = super::label(value, class);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_ellipsize(gtk::pango::EllipsizeMode::None);
    label.set_hexpand(true);
    label
}
fn show_name(ui: &Ui, feed: &str) -> String {
    ui.db
        .borrow()
        .podcast_shows
        .get(feed)
        .map(|show| show.title.clone())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| {
            url::Url::parse(feed)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned))
                .unwrap_or_else(|| "Podcast".into())
        })
}
pub(super) fn set_cover(
    picture: &gtk::Picture,
    assigned: &RefCell<Option<PathBuf>>,
    path: Option<PathBuf>,
    size: i32,
) {
    if *assigned.borrow() == path {
        return;
    }
    *assigned.borrow_mut() = path.clone();
    if let Some(path) = path {
        image_loader::replace_when_ready(picture, &path, size);
    } else {
        image_loader::cancel(picture);
        picture.set_paintable(None::<&gtk::gdk::Paintable>);
    }
}
fn cover_picture(
    ui: &Ui,
    feed: &str,
    picture: &gtk::Picture,
    assigned: &RefCell<Option<PathBuf>>,
    size: i32,
) {
    let path = ui
        .db
        .borrow()
        .podcast_shows
        .get(feed)
        .and_then(|show| show.image.as_deref())
        .map(media::podcast_cover_path)
        .filter(|path| path.is_file());
    set_cover(picture, assigned, path, size * ui.win.scale_factor());
}

fn artwork(size: i32) -> (gtk::Overlay, gtk::Picture) {
    let overlay = gtk::Overlay::new();
    let fallback = gtk::Image::from_icon_name("audio-x-generic-symbolic");
    fallback.set_pixel_size(48);
    overlay.set_child(Some(&fallback));
    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_can_shrink(true);
    overlay.add_overlay(&picture);
    overlay.set_size_request(size, size);
    overlay.add_css_class("podcast-artwork");
    (overlay, picture)
}
impl Ui {
    pub(super) fn podcast_gallery(self: &Rc<Self>, root: &gtk::Box) {
        root.set_spacing(12);
        let actions = row(12);
        let add = action(self, "Add a podcast", |ui| {
            ui.go(Page::Collection("podcasts/browse".into()));
            if !ui.podcast_browser.borrow().opened {
                ui.scan_podcast_list(podcasts::RECOMMENDATIONS.into());
            }
        });
        add.add_css_class("podcast-add");
        actions.append(&add);
        actions.append(&action(self, "Subscribe to feed", |ui| {
            ui.prompt("RSS or Atom feed URL", "Subscribe", |ui, feed| {
                ui.read_podcast(feed, true)
            })
        }));
        root.append(&actions);
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Find your podcasts"));
        root.append(&search);
        let count = label("", "collection-count");
        root.append(&count);
        let tiles = Rc::new(RefCell::new(HashMap::<String, Tile>::new()));
        let model = gtk::StringList::new(&[]);
        let factory = gtk::SignalListItemFactory::new();
        let ui = Rc::downgrade(self);
        let visible = tiles.clone();
        factory.connect_bind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let Some(feed) = item
                .item()
                .and_downcast::<gtk::StringObject>()
                .map(|value| value.string())
            else {
                return;
            };
            let Some(ui) = ui.upgrade() else {
                return;
            };
            let tile = ui.podcast_tile(&feed);
            item.set_child(Some(&tile.root));
            visible.borrow_mut().insert(feed.to_string(), tile);
            if let Some(gallery) = ui.podcast_grid.borrow().clone() {
                gallery.schedule_visible();
            }
        });
        let visible = tiles.clone();
        factory.connect_unbind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(child) = item.child() {
                visible
                    .borrow_mut()
                    .retain(|_, tile| tile.root.upcast_ref::<gtk::Widget>() != &child);
            }
            item.set_child(gtk::Widget::NONE);
        });
        let grid = gtk::GridView::new(
            Some(gtk::NoSelection::new(Some(model.clone()))),
            Some(factory),
        );
        grid.set_min_columns(1);
        grid.set_max_columns(6);
        grid.add_css_class("collection-album-grid");
        let scroll = gtk::ScrolledWindow::builder()
            .child(&grid)
            .vexpand(true)
            .hexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        root.append(&scroll);
        if !self.db.borrow().episodes.is_empty() {
            let saved = gtk::Expander::new(Some("Downloaded episodes"));
            let list = col(4);
            for (title, path) in self.db.borrow().episodes.clone() {
                list.append(&action(self, &title, move |ui| {
                    ui.open_embedded(&path, true)
                }));
            }
            saved.set_child(Some(&list));
            root.append(&saved);
        }
        let gallery = Rc::new(Gallery {
            root: root.clone(),
            ui: Rc::downgrade(self),
            model,
            query: Default::default(),
            urls: Default::default(),
            tiles,
            count,
            scroll: scroll.clone(),
            grid,
            visibility_pending: Cell::new(false),
        });
        let weak = Rc::downgrade(&gallery);
        search.connect_search_changed(move |entry| {
            if let Some(gallery) = weak.upgrade() {
                *gallery.query.borrow_mut() = entry.text().into();
                gallery.refresh();
            }
        });
        let weak = Rc::downgrade(&gallery);
        scroll.vadjustment().connect_value_changed(move |_| {
            if let Some(gallery) = weak.upgrade() {
                gallery.schedule_visible();
            }
        });
        *self.podcast_grid.borrow_mut() = Some(gallery.clone());
        gallery.refresh();
    }
    fn podcast_tile(self: &Rc<Self>, feed: &str) -> Tile {
        self.podcast_tile_at(feed, 164)
    }
    fn podcast_tile_at(self: &Rc<Self>, feed: &str, size: i32) -> Tile {
        let root = col(4);
        root.add_css_class("podcast-tile");
        root.set_focusable(true);
        root.update_property(&[gtk::accessible::Property::Label(&format!(
            "{} podcast",
            show_name(self, feed)
        ))]);
        let destination = feed.to_owned();
        let ui = Rc::downgrade(self);
        screens::tile_context_menu(&root, move |tile| {
            if let Some(ui) = ui.upgrade() {
                ui.podcast_tile_menu(tile, &destination);
            }
        });
        let destination = feed.to_owned();
        let open = action(self, "", move |ui| ui.open_podcast_show(&destination));
        open.add_css_class("collection-album");
        let (art, picture) = artwork(size);
        let clickable = col(4);
        clickable.append(&art);
        open.set_tooltip_text(Some(&format!("Open {}", show_name(self, feed))));
        let title = label(&show_name(self, feed), "track");
        title.set_lines(2);
        title.set_wrap(true);
        title.set_max_width_chars(1);
        title.set_tooltip_text(Some(&show_name(self, feed)));
        clickable.append(&title);
        let publisher = label("", "small");
        publisher.set_max_width_chars(1);
        clickable.append(&publisher);
        open.set_child(Some(&clickable));
        root.append(&open);
        let destination = feed.to_owned();
        root.append(&action(self, "Play latest", move |ui| {
            ui.play_podcast_latest(&destination)
        }));
        let art_path = RefCell::new(None);
        cover_picture(self, feed, &picture, &art_path, size);
        Tile {
            root,
            title,
            publisher,
            picture,
            art_path,
        }
    }
    fn podcast_tile_menu(self: &Rc<Self>, tile: &gtk::Box, feed: &str) {
        screens::close_tile_menu(tile);
        let popover = gtk::Popover::new();
        popover.set_parent(tile);
        popover.update_property(&[gtk::accessible::Property::Label(&format!(
            "Actions for {}",
            show_name(self, feed)
        ))]);
        let actions = col(2);
        for title in [
            "Open podcast",
            "Play latest",
            "Refresh episodes",
            "Unsubscribe",
        ] {
            let weak = popover.downgrade();
            let feed = feed.to_owned();
            let button = action(self, title, move |ui| {
                if let Some(popover) = weak.upgrade() {
                    popover.popdown();
                    if popover.parent().is_some() {
                        popover.unparent();
                    }
                }
                match title {
                    "Open podcast" => ui.open_podcast_show(&feed),
                    "Play latest" => ui.play_podcast_latest(&feed),
                    "Refresh episodes" => ui.read_podcast(feed.clone(), false),
                    _ => ui.unsubscribe_podcast(&feed),
                }
            });
            if title == "Unsubscribe" {
                button
                    .set_tooltip_text(Some("Remove the subscription and keep downloaded episodes"));
            }
            actions.append(&button);
        }
        popover.set_child(Some(&actions));
        popover.connect_closed(|popover| {
            if popover.parent().is_some() {
                popover.unparent();
            }
        });
        popover.popup();
    }

    pub(super) fn unsubscribe_podcast(self: &Rc<Self>, value: &str) {
        let Ok(feed) = podcasts::normalize_url(value) else {
            return;
        };
        let removed = self.db.borrow_mut().unsubscribe_podcast(&feed);
        podcasts::cancel_feed_requests(
            &feed,
            &mut self.podcast_subscriptions.borrow_mut(),
            &mut self.podcast_read_queue.borrow_mut(),
            &mut self.podcast_latest_pending.borrow_mut(),
        );
        if self
            .podcast_resume_pending
            .borrow()
            .as_ref()
            .is_some_and(|pending| pending.feed_url == feed)
        {
            self.podcast_resume_pending.borrow_mut().take();
        }
        if !removed {
            self.notice("This podcast is no longer subscribed.");
            return;
        }
        self.podcast_browser.borrow_mut().revision += 1;
        let same_show = |page: &Page| matches!(page, Page::Podcast(candidate) if podcasts::normalize_url(candidate).is_ok_and(|candidate| candidate == feed));
        let redirect = same_show(&self.page.borrow());
        let destination = Page::Collection("podcasts".into());
        {
            let mut history = self.history.borrow_mut();
            history.retain(|page| !same_show(page));
            if redirect {
                while history.last() == Some(&destination) {
                    history.pop();
                }
            }
            self.back.set_sensitive(!history.is_empty());
        }
        if redirect {
            self.page.replace(destination);
            self.render();
        } else {
            self.refresh_podcast_views(&feed);
        }
        self.notice("Unsubscribed. Downloaded episodes were kept.");
        self.persist();
    }

    pub(super) fn open_podcast_show(self: &Rc<Self>, feed: &str) {
        if self
            .podcast_latest_pending
            .borrow()
            .as_deref()
            .is_some_and(|pending| pending != feed)
        {
            self.podcast_latest_pending.borrow_mut().take();
        }
        self.go(Page::Podcast(feed.into()));
        if !feed_fresh(
            self.podcast_refreshed.borrow().get(feed).copied(),
            Instant::now(),
        ) {
            self.read_podcast(feed.into(), false);
        }
    }
    pub(super) fn ensure_podcast_metadata(self: &Rc<Self>, feed: &str) {
        self.request_podcast_cover(feed);
        if self.podcast_jobs.borrow().len() >= 4
            || self.podcast_jobs.borrow().contains(feed)
            || self.podcast_metadata_attempted.borrow().contains(feed)
        {
            return;
        }
        let needs = self
            .db
            .borrow()
            .podcast_shows
            .get(feed)
            .is_none_or(|show| show.title.is_empty() || show.image.is_none());
        if needs {
            self.podcast_metadata_attempted
                .borrow_mut()
                .insert(feed.into());
            self.read_podcast(feed.into(), false);
        }
    }
    pub(super) fn play_podcast_latest(self: &Rc<Self>, feed: &str) {
        self.podcast_resume_pending.borrow_mut().take();
        *self.podcast_latest_pending.borrow_mut() = Some(feed.into());
        self.open_podcast_show(feed);
        let latest = self
            .feeds
            .borrow()
            .get(feed)
            .and_then(|items| items.first())
            .cloned();
        if let Some(latest) = latest.filter(|_| {
            feed_fresh(
                self.podcast_refreshed.borrow().get(feed).copied(),
                Instant::now(),
            )
        }) {
            self.force_latest_podcast(feed, &latest);
        } else {
            *self.podcast_latest_pending.borrow_mut() = Some(feed.into());
            self.read_podcast(feed.into(), false);
        }
    }
    pub(super) fn podcast_show_page(self: &Rc<Self>, feed: &str, root: &gtk::Box) {
        root.set_spacing(12);
        root.add_css_class("podcast-show");
        let destination = feed.to_owned();
        root.append(&action(self, "← Your podcasts", |ui| {
            ui.go(Page::Collection("podcasts".into()))
        }));
        let header = row(18);
        let (cover, picture) = artwork(148);
        header.append(&cover);
        let names = col(6);
        names.set_hexpand(true);
        let title = text(&show_name(self, feed), "section");
        let publisher = text("", "podcast-publisher");
        names.append(&title);
        names.append(&publisher);
        let actions = row(8);
        let latest = action(self, "Play latest", move |ui| {
            ui.play_podcast_latest(&destination)
        });
        actions.append(&latest);
        let destination = feed.to_owned();
        let refresh = action(self, "Refresh episodes", move |ui| {
            ui.read_podcast(destination.clone(), false)
        });
        actions.append(&refresh);
        let destination = feed.to_owned();
        let subscription = action(self, "Unsubscribe", move |ui| {
            if podcasts::contains_feed(&ui.db.borrow().feeds, &destination) {
                ui.unsubscribe_podcast(&destination);
            } else {
                ui.read_podcast(destination.clone(), true);
            }
        });
        actions.append(&subscription);
        names.append(&actions);
        let order = self.podcast_order_picker(feed);
        names.append(&order);
        header.append(&names);
        root.append(&header);
        let synopsis = text("", "podcast-synopsis");
        synopsis.set_lines(5);
        synopsis.set_ellipsize(gtk::pango::EllipsizeMode::End);
        root.append(&synopsis);
        let full_summary = text("", "podcast-synopsis");
        full_summary.set_selectable(true);
        let synopsis_expander = gtk::Expander::new(Some("Full synopsis"));
        let synopsis_scroll = gtk::ScrolledWindow::builder()
            .child(&full_summary)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_height(64)
            .max_content_height(120)
            .propagate_natural_height(true)
            .build();
        synopsis_expander.set_child(Some(&synopsis_scroll));
        let compact = synopsis.downgrade();
        synopsis_expander.connect_expanded_notify(move |expander| {
            if let Some(compact) = compact.upgrade() {
                compact.set_visible(!expander.is_expanded());
            }
        });
        root.append(&synopsis_expander);
        let message = text("", "podcast-hint");
        root.append(&message);
        let data = Rc::new(RefCell::new(HashMap::<String, media::Episode>::new()));
        let rows = Rc::new(RefCell::new(HashMap::<String, EpisodeRow>::new()));
        let model = gtk::StringList::new(&[]);
        let factory = gtk::SignalListItemFactory::new();
        let ui = Rc::downgrade(self);
        let feed = feed.to_owned();
        let bound_feed = feed.clone();
        let bound_data = data.clone();
        let bound_rows = rows.clone();
        factory.connect_bind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let Some(url) = item
                .item()
                .and_downcast::<gtk::StringObject>()
                .map(|value| value.string())
            else {
                return;
            };
            if let Some(ui) = ui.upgrade()
                && let Some(episode) = bound_data.borrow().get(url.as_str())
            {
                let row = ui.podcast_episode_row(&bound_feed, episode);
                item.set_child(Some(&row.root));
                bound_rows.borrow_mut().insert(url.to_string(), row);
            }
        });
        let bound_rows = rows.clone();
        factory.connect_unbind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(child) = item.child() {
                bound_rows
                    .borrow_mut()
                    .retain(|_, row| row.root.upcast_ref::<gtk::Widget>() != &child);
            }
            item.set_child(gtk::Widget::NONE);
        });
        let list = gtk::ListView::new(
            Some(gtk::NoSelection::new(Some(model.clone()))),
            Some(factory),
        );
        list.add_css_class("podcast-list");
        root.append(
            &gtk::ScrolledWindow::builder()
                .child(&list)
                .vexpand(true)
                .hexpand(true)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .build(),
        );
        let show = Rc::new(Show {
            root: root.clone(),
            feed,
            ui: Rc::downgrade(self),
            title,
            publisher,
            summary: synopsis,
            full_summary,
            synopsis: synopsis_expander,
            picture,
            art_path: RefCell::new(None),
            message,
            latest,
            refresh,
            subscription,
            order,
            model,
            urls: Default::default(),
            data,
            rows,
        });
        *self.podcast_show.borrow_mut() = Some(show.clone());
        show.refresh();
        self.request_podcast_cover(&show.feed);
    }
    fn podcast_episode_row(self: &Rc<Self>, feed: &str, episode: &media::Episode) -> EpisodeRow {
        let root = col(6);
        root.add_css_class("podcast-episode");
        let heading = row(12);
        let title = text(&episode.title, "track");
        heading.append(&title);
        let selected = episode.url.clone();
        let destination = feed.to_owned();
        let play = action(self, "Play", move |ui| {
            let episode = ui
                .feeds
                .borrow()
                .get(&destination)
                .and_then(|episodes| episodes.iter().find(|episode| episode.url == selected))
                .cloned();
            if let Some(episode) = episode {
                ui.stream_podcast(&destination, &episode);
            }
        });
        heading.append(&play);
        let selected = episode.clone();
        let download_feed = feed.to_owned();
        heading.append(&action(self, "Download", move |ui| {
            let episode = selected.clone();
            let tx = ui.tx.clone();
            let id = podcasts::EpisodeId::new(&download_feed, &episode.url).unwrap();
            ui.notice("Downloading episode…");
            std::thread::spawn(move || {
                let result = media::download(&episode, &data_dir().join("episodes"));
                let _ = tx.send(Event::PodcastDownloaded(id, episode.title, result));
            });
        }));
        root.append(&heading);
        let date = label(&episode_date(episode), "small");
        root.append(&date);
        let summary = text(&episode.summary, "podcast-episode-summary");
        summary.set_lines(3);
        summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
        root.append(&summary);
        EpisodeRow {
            root,
            title,
            summary,
            date,
        }
    }
    pub(super) fn podcast_order_picker(self: &Rc<Self>, feed: &str) -> gtk::DropDown {
        let picker = gtk::DropDown::from_strings(&["Newest first", "Oldest first"]);
        let selected = self
            .db
            .borrow()
            .podcast_orders
            .get(feed)
            .copied()
            .unwrap_or_default();
        picker.set_selected(if selected == podcasts::EpisodeOrder::OldestFirst {
            1
        } else {
            0
        });
        picker.set_tooltip_text(Some("Episode and playback order"));
        picker.update_property(&[gtk::accessible::Property::Label(
            "Episode and playback order",
        )]);
        let destination = feed.to_owned();
        let ui = Rc::downgrade(self);
        picker.connect_selected_notify(move |picker| {
            if let Some(ui) = ui.upgrade() {
                let order = if picker.selected() == 1 {
                    podcasts::EpisodeOrder::OldestFirst
                } else {
                    podcasts::EpisodeOrder::NewestFirst
                };
                let previous = ui
                    .db
                    .borrow()
                    .podcast_orders
                    .get(&destination)
                    .copied()
                    .unwrap_or_default();
                if previous != order {
                    ui.db
                        .borrow_mut()
                        .podcast_orders
                        .insert(destination.clone(), order);
                    ui.reorder_playing_podcast(&destination);
                    ui.refresh_podcast_views(&destination);
                    ui.persist();
                }
            }
        });
        picker
    }

    pub(super) fn resume_last_podcast(self: &Rc<Self>) {
        let Some(last) = self.db.borrow().last_podcast.clone() else {
            self.notice("Play a podcast episode to start its listening history.");
            return;
        };
        self.podcast_latest_pending.borrow_mut().take();
        *self.podcast_resume_pending.borrow_mut() = Some(last.clone());
        if self.feeds.borrow().contains_key(&last.feed_url) {
            self.finish_podcast_resume(&last.feed_url, true);
        } else {
            self.read_podcast(last.feed_url.clone(), false);
        }
    }
    pub(super) fn finish_podcast_resume(self: &Rc<Self>, feed: &str, final_read: bool) {
        let last = self.podcast_resume_pending.borrow().clone();
        let Some(last) = last.filter(|last| last.feed_url == feed) else {
            return;
        };
        let order = self
            .db
            .borrow()
            .podcast_orders
            .get(feed)
            .copied()
            .unwrap_or_default();
        let episodes = podcasts::ordered_episodes(
            self.feeds.borrow().get(feed).map_or(&[], Vec::as_slice),
            order,
        );
        let progress = self
            .db
            .borrow()
            .podcast_progress
            .get(feed)
            .cloned()
            .unwrap_or_default();
        if let Some((index, position)) =
            podcasts::resume_episode(&episodes, &last.episode_url, &progress)
        {
            let episode = episodes[index].clone();
            self.podcast_resume_pending.borrow_mut().take();
            self.resume_podcast_episode(feed, &episode, position);
        } else if final_read {
            self.podcast_resume_pending.borrow_mut().take();
            self.open_podcast_show(feed);
            self.notice("No unfinished episodes are available. Choose an episode to replay it.");
        }
    }
    pub(super) fn podcast_listening_page(self: &Rc<Self>) {
        let active = self
            .media
            .borrow()
            .as_ref()
            .and_then(|media| media.podcast_id.clone().map(|id| (id, media.title.clone())));
        let Some((id, episode_title)) = active else {
            self.hero(false, false);
            return;
        };
        let root = if self.compact.get() { col(18) } else { row(34) };
        root.add_css_class("hero");
        root.set_halign(gtk::Align::Center);
        root.set_valign(gtk::Align::Center);
        root.set_vexpand(true);
        let size = if self.compact.get() { 175 } else { 295 };
        let (art, picture) = artwork(size);
        let assigned = RefCell::new(None);
        cover_picture(self, &id.feed_url, &picture, &assigned, size);
        root.append(&art);
        let info = col(10);
        info.set_valign(gtk::Align::Center);
        info.set_hexpand(true);
        info.append(&label("NOW PLAYING · PODCAST", "eyebrow"));
        let title = text(&episode_title, "hero-title");
        title.set_max_width_chars(22);
        info.append(&title);
        info.append(&text(&show_name(self, &id.feed_url), "hero-artist"));
        if let Some(show) = self.db.borrow().podcast_shows.get(&id.feed_url) {
            info.append(&text(&show.publisher, "hero-album"));
        }
        let feed = id.feed_url.clone();
        info.append(&action(self, "Show episodes", move |ui| {
            ui.open_podcast_show(&feed)
        }));
        info.append(&self.podcast_order_picker(&id.feed_url));
        root.append(&info);
        self.body.append(&root);
    }
    pub(super) fn quickplay_podcast_shelf(self: &Rc<Self>, root: &gtk::Box, size: i32) -> bool {
        let mut feeds: Vec<_> = self
            .db
            .borrow()
            .feeds
            .iter()
            .filter_map(|feed| podcasts::normalize_url(feed).ok())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        feeds.sort_by_cached_key(|feed| show_name(self, feed).to_lowercase());
        if feeds.is_empty() {
            return false;
        }
        let tiles = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .min_children_per_line(1)
            .max_children_per_line(12)
            .column_spacing(16)
            .row_spacing(16)
            .build();
        for feed in feeds.into_iter().take(8) {
            let tile = self.podcast_tile_at(&feed, size);
            tile.root.set_width_request(size + 16);
            tiles.insert(&tile.root, -1);
        }
        root.append(&tiles);
        true
    }
    pub(super) fn quickplay_podcast(self: &Rc<Self>, root: &gtk::Box, size: i32) {
        let Some(last) = self.db.borrow().last_podcast.clone() else {
            root.append(&text(
                "Your latest podcast appears here after you start an episode.",
                "small",
            ));
            root.append(&action(self, "Browse podcasts", |ui| {
                ui.go(Page::Collection("podcasts".into()))
            }));
            return;
        };
        let resume = action(self, "", |ui| ui.resume_last_podcast());
        let content = col(6);
        let (art, picture) = artwork(size);
        let assigned = RefCell::new(None);
        cover_picture(self, &last.feed_url, &picture, &assigned, size);
        content.append(&art);
        let name = text(&show_name(self, &last.feed_url), "track");
        name.set_max_width_chars((size / 8).max(18));
        content.append(&name);
        let progress = self
            .db
            .borrow()
            .podcast_progress
            .get(&last.feed_url)
            .and_then(|episodes| episodes.get(&last.episode_url))
            .copied()
            .unwrap_or_default();
        content.append(&text(
            &if progress.completed {
                "Continue podcast".into()
            } else {
                format!("Resume at {}", time(progress.position_secs))
            },
            "small",
        ));
        resume.set_child(Some(&content));
        resume.set_tooltip_text(Some(
            "Resume your latest podcast episode and listening order",
        ));
        root.append(&resume);
    }
    pub(super) fn refresh_podcast_views(self: &Rc<Self>, feed: &str) {
        if let Some(gallery) = self.podcast_grid.borrow().clone() {
            gallery.refresh();
        }
        if let Some(show) = self.podcast_show.borrow().clone()
            && show.feed == feed
        {
            show.refresh();
        }
    }
    pub(super) fn request_podcast_cover(self: &Rc<Self>, feed: &str) {
        let image = self
            .db
            .borrow()
            .podcast_shows
            .get(feed)
            .and_then(|show| show.image.clone())
            .or_else(|| {
                self.podcast_browser
                    .borrow()
                    .shows
                    .iter()
                    .find(|show| show.url == feed)
                    .and_then(|show| show.show.image.clone())
            });
        let Some(image) = image else {
            return;
        };
        self.request_podcast_cover_url(feed, &image);
    }
    pub(super) fn request_podcast_cover_url(self: &Rc<Self>, feed: &str, image: &str) {
        let image = image.to_owned();
        if media::podcast_cover_path(&image).is_file()
            || self.podcast_cover_jobs.borrow().len() >= 2
            || self.podcast_cover_attempted.borrow().contains(&image)
        {
            return;
        }
        self.podcast_cover_jobs.borrow_mut().insert(image.clone());
        self.podcast_cover_attempted
            .borrow_mut()
            .insert(image.clone());
        let feed = feed.to_owned();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = media::podcast_cover(&image);
            let _ = tx.send(Event::PodcastCover(feed, image, result));
        });
    }
}
impl Gallery {
    fn refresh(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let query = self.query.borrow().to_lowercase();
        let mut feeds: Vec<_> = ui
            .db
            .borrow()
            .feeds
            .iter()
            .filter_map(|feed| podcasts::normalize_url(feed).ok())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        feeds.retain(|feed| {
            let show = ui
                .db
                .borrow()
                .podcast_shows
                .get(feed)
                .cloned()
                .unwrap_or_default();
            format!(
                "{} {} {} {}",
                show_name(&ui, feed),
                show.publisher,
                show.summary,
                feed
            )
            .to_lowercase()
            .contains(&query)
        });
        feeds.sort_by_cached_key(|feed| (show_name(&ui, feed).to_lowercase(), feed.clone()));
        self.count.set_text(if feeds.is_empty() {
            "No podcasts match. Add a podcast to start your collection."
        } else {
            "Your podcasts"
        });
        let changed = *self.urls.borrow() != feeds;
        if changed {
            *self.urls.borrow_mut() = feeds.clone();
            let values: Vec<_> = feeds.iter().map(String::as_str).collect();
            self.model.splice(0, self.model.n_items(), &values);
        }
        let visible: Vec<_> = self.tiles.borrow().keys().cloned().collect();
        for feed in visible {
            if let Some(tile) = self.tiles.borrow().get(&feed) {
                tile.title.set_text(&show_name(&ui, &feed));
                let publisher = ui
                    .db
                    .borrow()
                    .podcast_shows
                    .get(&feed)
                    .map(|show| show.publisher.clone())
                    .unwrap_or_default();
                tile.publisher.set_text(&publisher);
                cover_picture(&ui, &feed, &tile.picture, &tile.art_path, 164);
            }
        }
        self.schedule_visible();
    }
    fn schedule_visible(self: &Rc<Self>) {
        if self.visibility_pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        after_layout(&self.grid, move || {
            if let Some(view) = weak.upgrade() {
                view.visibility_pending.set(false);
                let Some(ui) = view.ui.upgrade() else {
                    return;
                };
                let feeds: Vec<_> = view
                    .tiles
                    .borrow()
                    .iter()
                    .filter(|(_, tile)| {
                        tile.root.is_mapped()
                            && tile
                                .root
                                .compute_bounds(&view.scroll)
                                .is_some_and(|bounds| {
                                    bounds.width() > 0.
                                        && bounds.height() > 0.
                                        && bounds.y() + bounds.height() > 0.
                                        && bounds.y() < view.scroll.height() as f32
                                })
                    })
                    .map(|(feed, _)| feed.clone())
                    .collect();
                for feed in feeds {
                    ui.ensure_podcast_metadata(&feed);
                }
            }
        });
    }
}
impl Show {
    fn refresh(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let show = ui
            .db
            .borrow()
            .podcast_shows
            .get(&self.feed)
            .cloned()
            .unwrap_or_default();
        self.title.set_text(&show_name(&ui, &self.feed));
        self.publisher.set_text(&show.publisher);
        self.summary.set_text(&show.summary);
        self.full_summary.set_text(&show.summary);
        self.synopsis
            .set_visible(show.summary.chars().count() > 300);
        self.summary
            .set_visible(!show.summary.is_empty() && !self.synopsis.is_expanded());
        cover_picture(&ui, &self.feed, &self.picture, &self.art_path, 148);
        let busy = ui.podcast_jobs.borrow().contains(&self.feed)
            || ui
                .podcast_read_queue
                .borrow()
                .iter()
                .any(|(feed, _, _)| feed == &self.feed);
        self.refresh.set_sensitive(!busy);
        let subscribed = podcasts::contains_feed(&ui.db.borrow().feeds, &self.feed);
        let subscribing = ui.podcast_subscriptions.borrow().contains(&self.feed)
            || ui
                .podcast_read_queue
                .borrow()
                .iter()
                .any(|(feed, subscribe, _)| feed == &self.feed && *subscribe);
        self.subscription.set_label(if subscribed {
            "Unsubscribe"
        } else if subscribing {
            "Subscribing…"
        } else {
            "Subscribe"
        });
        self.subscription.set_sensitive(subscribed || !subscribing);
        self.subscription.set_tooltip_text(Some(if subscribed {
            "Remove the subscription and keep downloaded episodes"
        } else {
            "Add this show to Your podcasts"
        }));
        let order = ui
            .db
            .borrow()
            .podcast_orders
            .get(&self.feed)
            .copied()
            .unwrap_or_default();
        self.order
            .set_selected(if order == podcasts::EpisodeOrder::OldestFirst {
                1
            } else {
                0
            });
        let episodes = podcasts::ordered_episodes(
            ui.feeds.borrow().get(&self.feed).map_or(&[], Vec::as_slice),
            order,
        );
        self.latest.set_sensitive(!episodes.is_empty());
        let error = ui
            .podcast_browser
            .borrow()
            .feed_errors
            .get(&self.feed)
            .cloned();
        let status = if let Some(error) = error {
            error
        } else if busy {
            "Refreshing episodes…".into()
        } else if episodes.is_empty() {
            "No playable episodes are currently listed.".into()
        } else {
            format!("{} episodes", episodes.len())
        };
        self.message.set_text(&status);
        let urls: Vec<_> = episodes.iter().map(|episode| episode.url.clone()).collect();
        *self.data.borrow_mut() = episodes
            .into_iter()
            .map(|episode| (episode.url.clone(), episode))
            .collect();
        if *self.urls.borrow() != urls {
            *self.urls.borrow_mut() = urls.clone();
            let values: Vec<_> = urls.iter().map(String::as_str).collect();
            self.model.splice(0, self.model.n_items(), &values);
        }
        ui.request_podcast_cover(&self.feed);
        for (url, row) in self.rows.borrow().iter() {
            if let Some(episode) = self.data.borrow().get(url) {
                row.title.set_text(&episode.title);
                row.summary.set_text(&episode.summary);
                let completed = ui
                    .db
                    .borrow()
                    .podcast_progress
                    .get(&self.feed)
                    .and_then(|progress| progress.get(url))
                    .is_some_and(|saved| saved.completed);
                row.date.set_text(&if completed {
                    format!("Completed · {}", episode_date(episode))
                } else {
                    episode_date(episode)
                });
            }
        }
    }
}
fn episode_date(episode: &media::Episode) -> String {
    episode
        .published
        .and_then(|timestamp| chrono::DateTime::from_timestamp(timestamp, 0))
        .map(|date| date.format("%b %-d, %Y").to_string())
        .unwrap_or_default()
}

pub(super) fn after_layout(widget: &impl IsA<gtk::Widget>, callback: impl FnOnce() + 'static) {
    let callback = RefCell::new(Some(callback));
    let allocated = Cell::new(false);
    widget.add_tick_callback(move |widget, _| {
        if !widget.is_mapped() {
            return glib::ControlFlow::Break;
        }
        if !allocated.replace(true) {
            return glib::ControlFlow::Continue;
        }
        if let Some(callback) = callback.borrow_mut().take() {
            callback();
        }
        glib::ControlFlow::Break
    });
}

fn feed_fresh(refreshed: Option<Instant>, now: Instant) -> bool {
    refreshed.is_some_and(|refreshed| {
        now.saturating_duration_since(refreshed) < Duration::from_secs(15 * 60)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn latest_episode_freshness_expires_during_a_long_desktop_session() {
        let now = Instant::now();
        assert!(!feed_fresh(None, now));
        assert!(feed_fresh(Some(now), now + Duration::from_secs(899)));
        assert!(!feed_fresh(Some(now), now + Duration::from_secs(900)));
    }
}
