use super::*;

pub(super) struct PodcastBrowser {
    root: gtk::Box,
    search: gtk::SearchEntry,
    source: gtk::Label,
    message: gtk::Label,
    count: gtk::Label,
    empty: gtk::Label,
    controls: Vec<gtk::Button>,
    model: gtk::StringList,
    list: gtk::ListView,
    scroll: gtk::ScrolledWindow,
    rows: Rc<RefCell<HashMap<String, Rc<PodcastRow>>>>,
    data: Rc<RefCell<HashMap<String, Rc<podcasts::Listing>>>>,
    urls: RefCell<Vec<String>>,
    key: RefCell<Option<podcasts::ResultKey>>,
    interaction: Rc<Cell<u64>>,
    layout_generation: Cell<u64>,
    cover_check_pending: Cell<bool>,
}
struct PodcastRow {
    root: gtk::Box,
    art: gtk::Overlay,
    picture: gtk::Picture,
    art_path: RefCell<Option<PathBuf>>,
    title: gtk::Label,
    publisher: gtk::Label,
    summary: gtk::Label,
    full: gtk::Expander,
    full_text: gtk::Label,
    subscribe: gtk::Button,
    read: gtk::Button,
    count: gtk::Label,
    error: gtk::Label,
}

impl Ui {
    pub(super) fn podcast_browser_mounted(&self) -> bool {
        self.podcast_browser
            .borrow()
            .view
            .as_ref()
            .is_some_and(|view| view.root.parent().is_some())
    }

    pub(super) fn podcast_browser(self: &Rc<Self>, list: &gtk::Box) {
        list.set_spacing(12);
        let heading = row(12);
        heading.append(&label("add a podcast", "section"));
        heading.append(&gap());
        heading.append(&podcast_button(self, "Your podcasts", |u| {
            u.go(Page::Collection("podcasts".into()))
        }));
        list.append(&heading);
        let directory = row(8);
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search shows, publishers and summaries"));
        search.set_hexpand(true);
        search.set_text(&self.podcast_browser.borrow().query);
        directory.append(&search);
        let entry = search.downgrade();
        let global = podcast_button(self, "Search full directory", move |u| {
            if let Some(entry) = entry.upgrade() {
                u.search_podcast_directory(entry.text().trim().into());
            }
        });
        directory.append(&global);
        let mut controls = vec![global];
        let ui = Rc::downgrade(self);
        search.connect_activate(move |entry| {
            if let Some(ui) = ui.upgrade() {
                ui.search_podcast_directory(entry.text().trim().into());
            }
        });
        list.append(&directory);
        let tools = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .max_children_per_line(5)
            .min_children_per_line(1)
            .column_spacing(8)
            .row_spacing(6)
            .build();
        for (title, source) in [
            ("Podcast Index recommendations", podcasts::RECOMMENDATIONS),
            ("BBC feed list", podcasts::BBC),
            ("Starter picks", podcasts::STARTER),
        ] {
            let b = podcast_button(self, title, move |u| u.scan_podcast_list(source.into()));
            tools.insert(&b, -1);
            controls.push(b);
        }
        let scan = podcast_button(self, "Scan feed list", |u| {
            u.prompt("OPML URL or local file path", "Scan list", |u, source| {
                u.scan_podcast_list(source)
            });
        });
        tools.insert(&scan, -1);
        controls.push(scan);
        let file = podcast_button(self, "Open OPML file", |u| {
            let ui = Rc::downgrade(u);
            gtk::FileDialog::builder()
                .title("Open a podcast feed list")
                .build()
                .open(Some(&u.win), None::<&gio::Cancellable>, move |result| {
                    if let Some(ui) = ui.upgrade()
                        && let Ok(file) = result
                        && let Some(path) = file.path()
                    {
                        ui.scan_podcast_list(path.to_string_lossy().into());
                    }
                });
        });
        tools.insert(&file, -1);
        controls.push(file);
        tools.insert(
            &podcast_button(self, "Preview RSS / Atom", |u| {
                u.prompt("RSS or Atom feed URL", "Preview feed", |u, feed| {
                    u.preview_podcast(feed)
                });
            }),
            -1,
        );
        list.append(&tools);
        let source = podcast_text("", "podcast-hint");
        let message = podcast_text("", "podcast-hint");
        let count = label("", "collection-count");
        let empty = podcast_text("", "podcast-hint");
        for value in [&source, &message, &count, &empty] {
            list.append(value);
        }
        let model = gtk::StringList::new(&[]);
        let data = Rc::new(RefCell::new(HashMap::<String, Rc<podcasts::Listing>>::new()));
        let rows = Rc::new(RefCell::new(HashMap::<String, Rc<PodcastRow>>::new()));
        let factory = gtk::SignalListItemFactory::new();
        let ui = Rc::downgrade(self);
        let bound_data = data.clone();
        let bound_rows = rows.clone();
        factory.connect_bind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let Some(url) = item
                .item()
                .and_downcast::<gtk::StringObject>()
                .map(|obj| obj.string())
            else {
                return;
            };
            if let Some(ui) = ui.upgrade()
                && let Some(listing) = bound_data.borrow().get(url.as_str())
            {
                let row = Rc::new(ui.podcast_listing(listing));
                item.set_child(Some(&row.root));
                bound_rows.borrow_mut().insert(url.to_string(), row);
                ui.schedule_podcast_browser_covers();
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
        let view = gtk::ListView::new(
            Some(gtk::NoSelection::new(Some(model.clone()))),
            Some(factory),
        );
        view.add_css_class("podcast-list");
        let scroll = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .hexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&view)
            .build();
        list.append(&scroll);
        let interaction = Rc::new(Cell::new(0_u64));
        let input = gtk::EventControllerLegacy::new();
        input.set_propagation_phase(gtk::PropagationPhase::Capture);
        let activity = interaction.clone();
        input.connect_event(move |_, event| {
            use gtk::gdk::EventType;
            if matches!(
                event.event_type(),
                EventType::Scroll
                    | EventType::ButtonPress
                    | EventType::KeyPress
                    | EventType::TouchBegin
                    | EventType::TouchUpdate
                    | EventType::TouchpadSwipe
            ) || (event.event_type() == EventType::MotionNotify
                && event
                    .modifier_state()
                    .contains(gtk::gdk::ModifierType::BUTTON1_MASK))
            {
                activity.set(activity.get().wrapping_add(1));
            }
            glib::Propagation::Proceed
        });
        list.add_controller(input);
        self.podcast_browser.borrow_mut().view = Some(Rc::new(PodcastBrowser {
            root: list.clone(),
            search: search.clone(),
            source,
            message,
            count,
            empty,
            controls,
            model,
            list: view,
            scroll,
            rows,
            data,
            urls: Default::default(),
            key: Default::default(),
            interaction,
            layout_generation: Cell::new(0),
            cover_check_pending: Cell::new(false),
        }));
        // Capture typed text immediately; SearchEntry's delayed signal only applies the filter.
        let ui = Rc::downgrade(self);
        search.connect_changed(move |entry| {
            if let Some(ui) = ui.upgrade() {
                let mut browse = ui.podcast_browser.borrow_mut();
                if browse
                    .view
                    .as_ref()
                    .is_some_and(|view| view.search == *entry)
                {
                    browse.query = entry.text().into();
                }
            }
        });
        let ui = Rc::downgrade(self);
        search.connect_search_changed(move |entry| {
            if let Some(ui) = ui.upgrade()
                && ui
                    .podcast_browser
                    .borrow()
                    .view
                    .as_ref()
                    .is_some_and(|view| view.search == *entry)
            {
                ui.refresh_podcast_browser();
            }
        });
        let weak = Rc::downgrade(self);
        self.podcast_browser
            .borrow()
            .view
            .as_ref()
            .unwrap()
            .scroll
            .vadjustment()
            .connect_value_changed(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.schedule_podcast_browser_covers();
                }
            });
        self.refresh_podcast_browser();
    }

    pub(super) fn refresh_podcast_browser(self: &Rc<Self>) {
        let browse = self.podcast_browser.borrow();
        let Some(view) = browse.view.clone() else {
            return;
        };
        let key = podcasts::ResultKey {
            revision: browse.revision,
            query: browse.query.clone(),
        };
        let reset = view.key.borrow().as_ref() != Some(&key);
        let source = if browse.source == podcasts::RECOMMENDATIONS {
            "Podcast Index recommendations"
        } else if browse.source == podcasts::BBC {
            "BBC public podcast feed list"
        } else {
            &browse.source
        };
        view.source.set_text(source);
        view.message.set_text(&browse.message);
        view.message.set_visible(!browse.message.is_empty());
        for control in &view.controls {
            control.set_sensitive(!browse.scanning);
        }
        view.empty.set_text(if browse.shows.is_empty() {
            "No unsubscribed shows to browse. Try Starter picks, search the full directory or scan another OPML feed list."
        } else { "No unsubscribed shows match this search. Try another title, search the full directory or scan a different list." });
        if !reset {
            view.empty
                .set_visible(view.urls.borrow().is_empty() && !browse.scanning);
            return;
        }
        let subscribed: HashSet<_> = self
            .db
            .borrow()
            .feeds
            .iter()
            .filter_map(|s| podcasts::normalize_url(s).ok())
            .collect();
        let mut matching: Vec<_> = browse
            .shows
            .iter()
            .filter(|s| !subscribed.contains(&s.url) && podcasts::matches(s, &browse.query))
            .collect();
        if !browse.query.trim().is_empty() {
            matching.sort_by_cached_key(|listing| {
                (
                    podcasts::search_rank(listing, &browse.query),
                    listing.show.title.to_lowercase(),
                )
            });
        }
        let urls: Vec<_> = matching.iter().map(|listing| listing.url.clone()).collect();
        *view.data.borrow_mut() = matching
            .iter()
            .map(|listing| (listing.url.clone(), (*listing).clone()))
            .collect();
        view.count.set_text(&format!(
            "{} unsubscribed shows · {} subscribed",
            urls.len(),
            subscribed.len()
        ));
        view.empty.set_visible(urls.is_empty() && !browse.scanning);
        // GTK's model signals bind synchronously, and Editable::changed can update Browse.
        drop(browse);
        let additions: Vec<_> = urls.iter().map(String::as_str).collect();
        // Query/catalog changes deliberately replace results, even when their URLs
        // overlap: the new catalog can carry different titles and summaries.
        view.layout_generation
            .set(view.layout_generation.get().wrapping_add(1));
        view.model.splice(0, view.model.n_items(), &additions);
        *view.urls.borrow_mut() = urls;
        if !view.urls.borrow().is_empty() {
            // The item-aware GTK operation also works before virtual rows are allocated.
            view.list.scroll_to(0, gtk::ListScrollFlags::NONE, None);
        }
        if view.search.text().as_str() != key.query {
            view.search.set_text(&key.query);
        }
        *view.key.borrow_mut() = Some(key);
    }

    fn update_podcast_browser_feed(self: &Rc<Self>, feed: &str) {
        let browse = self.podcast_browser.borrow();
        let Some(view) = browse.view.clone() else {
            return;
        };
        if view
            .key
            .borrow()
            .as_ref()
            .is_none_or(|key| key.revision != browse.revision)
        {
            drop(browse);
            self.refresh_podcast_browser();
            return;
        }
        let listing = browse
            .shows
            .iter()
            .find(|listing| listing.url == feed)
            .cloned();
        drop(browse);
        let subscribed = podcasts::contains_feed(&self.db.borrow().feeds, feed);
        if subscribed {
            let position = view.urls.borrow().iter().position(|url| url == feed);
            if let Some(position) = position {
                // GTK remembers a list item's keyboard focus even after the user
                // scrolls it away. Anchor deletion to the viewport at completion,
                // not the row that was clicked when the request began.
                let rows = view.rows.borrow();
                let anchor = rows
                    .iter()
                    .filter(|(url, _)| url.as_str() != feed)
                    .filter_map(|(url, row)| {
                        podcast_row_bounds(row, &view.scroll)
                            .map(|bounds| (bounds.y(), url.clone(), row.clone()))
                    })
                    .min_by(|(a, _, _), (b, _, _)| a.total_cmp(b));
                let focus = gtk::prelude::RootExt::focus(&self.win);
                let list_focus = focus.as_ref().is_some_and(|focus| {
                    focus == view.list.upcast_ref::<gtk::Widget>() || focus.is_ancestor(&view.list)
                });
                let surviving_focus = rows.iter().any(|(url, row)| {
                    url != feed
                        && podcast_row_bounds(row, &view.scroll).is_some()
                        && row.root.parent().is_some_and(|item| {
                            focus
                                .as_ref()
                                .is_some_and(|focus| focus == &item || focus.is_ancestor(&item))
                        })
                });
                let removed_focus = rows.get(feed).is_some_and(|row| {
                    podcast_row_bounds(row, &view.scroll).is_some()
                        && row.root.parent().is_some_and(|item| {
                            focus
                                .as_ref()
                                .is_some_and(|focus| focus == &item || focus.is_ancestor(&item))
                        })
                });
                drop(rows);
                if list_focus
                    && !surviving_focus
                    && let Some((_, _, anchor)) = &anchor
                {
                    anchor.root.grab_focus();
                }
                view.urls.borrow_mut().remove(position);
                view.layout_generation
                    .set(view.layout_generation.get().wrapping_add(1));
                view.model.remove(position as u32);
                if let Some((offset, url, _)) = anchor {
                    preserve_podcast_anchor(&view, url, offset);
                } else if removed_focus && !view.urls.borrow().is_empty() {
                    // An expanded card can occupy the whole viewport. With no
                    // surviving visible anchor, stay near its successor.
                    let next = position.min(view.urls.borrow().len() - 1);
                    view.list
                        .scroll_to(next as u32, gtk::ListScrollFlags::NONE, None);
                }
                view.data.borrow_mut().remove(feed);
            }
            view.count.set_text(&format!(
                "{} unsubscribed shows · {} subscribed",
                view.urls.borrow().len(),
                self.db.borrow().feeds.len()
            ));
            view.empty.set_visible(
                view.urls.borrow().is_empty() && !self.podcast_browser.borrow().scanning,
            );
        } else if let Some(listing) = listing {
            view.data
                .borrow_mut()
                .insert(feed.to_owned(), listing.clone());
            let row = view.rows.borrow().get(feed).cloned();
            if let Some(row) = row {
                self.update_podcast_listing(&row, &listing);
            }
        }
    }

    fn podcast_listing(self: &Rc<Self>, listing: &podcasts::Listing) -> PodcastRow {
        let root = col(6);
        root.add_css_class("podcast-card");
        root.set_focusable(true);
        let title = podcast_text("", "section");
        let publisher = podcast_text("", "podcast-publisher");
        let summary = podcast_text("", "podcast-summary");
        summary.set_lines(3);
        summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let full = gtk::Expander::new(Some("Full summary"));
        let full_text = podcast_text("", "podcast-summary");
        full_text.set_selectable(true);
        full.set_child(Some(&full_text));
        let heading = row(12);
        let art = gtk::Overlay::new();
        art.set_size_request(76, 76);
        let fallback = gtk::Image::from_icon_name("audio-x-generic-symbolic");
        fallback.set_pixel_size(28);
        art.set_child(Some(&fallback));
        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_can_shrink(true);
        art.add_overlay(&picture);
        heading.append(&art);
        let names = col(6);
        names.set_hexpand(true);
        for value in [&title, &publisher, &summary] {
            names.append(value);
        }
        names.append(&full);
        heading.append(&names);
        root.append(&heading);
        let actions = row(8);
        let url = listing.url.clone();
        let subscribe = podcast_button(self, "Subscribe", move |u| {
            u.read_podcast(url.clone(), true)
        });
        let url = listing.url.clone();
        let read = podcast_button(self, "Read feed", move |u| {
            u.read_podcast(url.clone(), false)
        });
        let count = label("", "podcast-hint");
        actions.append(&subscribe);
        actions.append(&read);
        actions.append(&count);
        root.append(&actions);
        let error = podcast_text("", "podcast-error");
        root.append(&error);
        let address = gtk::Expander::new(Some("Feed address"));
        let value = podcast_text(&listing.url, "podcast-hint");
        value.set_selectable(true);
        address.set_child(Some(&value));
        root.append(&address);
        let row = PodcastRow {
            root,
            art,
            picture,
            art_path: RefCell::new(None),
            title,
            publisher,
            summary,
            full,
            full_text,
            subscribe,
            read,
            count,
            error,
        };
        self.update_podcast_listing(&row, listing);
        row
    }

    fn update_podcast_listing(&self, row: &PodcastRow, listing: &podcasts::Listing) {
        let browse = self.podcast_browser.borrow();
        row.art.set_visible(listing.show.image.is_some());
        let cover = listing
            .show
            .image
            .as_deref()
            .map(media::podcast_cover_path)
            .filter(|path| path.is_file());
        crate::podcast_collection::set_cover(
            &row.picture,
            &row.art_path,
            cover,
            76 * self.win.scale_factor(),
        );
        row.title.set_text(&listing.show.title);
        row.publisher.set_text(&listing.show.publisher);
        row.publisher
            .set_visible(!listing.show.publisher.is_empty());
        row.summary.set_text(if listing.show.summary.is_empty() {
            "Choose Read feed for the publisher’s summary and episode details."
        } else {
            &listing.show.summary
        });
        row.full_text.set_text(&listing.show.summary);
        row.full
            .set_visible(listing.show.summary.chars().count() > 220);
        let busy = self.podcast_jobs.borrow().contains(&listing.url);
        row.subscribe.set_label(
            if self.podcast_subscriptions.borrow().contains(&listing.url) {
                "Subscribing…"
            } else {
                "Subscribe"
            },
        );
        row.subscribe.set_sensitive(!busy);
        row.read
            .set_label(if busy { "Reading…" } else { "Read feed" });
        row.read.set_sensitive(!busy);
        let read = browse.read.contains(&listing.url);
        row.count.set_visible(read);
        if read {
            row.count.set_text(&format!(
                "{} playable episodes",
                self.feeds.borrow().get(&listing.url).map_or(0, Vec::len)
            ));
        }
        let error = browse.feed_errors.get(&listing.url);
        row.error.set_text(error.map_or("", String::as_str));
        row.error.set_visible(error.is_some());
    }

    fn search_podcast_directory(self: &Rc<Self>, query: String) {
        if query.is_empty() || self.podcast_browser.borrow().scanning {
            return;
        }
        {
            let mut browse = self.podcast_browser.borrow_mut();
            browse.opened = true;
            browse.scanning = true;
            browse.query = query.clone();
            browse.message = format!("Searching p0dcasters for {query}…");
        }
        self.refresh_podcast_browser();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = podcasts::search_directory(&query)
                .map(|catalog| (format!("p0dcasters directory: {query}"), catalog));
            let _ = tx.send(Event::PodcastCatalog(result));
        });
    }

    pub(super) fn schedule_podcast_browser_covers(self: &Rc<Self>) {
        let Some(view) = self.podcast_browser.borrow().view.clone() else {
            return;
        };
        if view.cover_check_pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        let target = Rc::downgrade(&view);
        crate::podcast_collection::after_layout(&view.list, move || {
            if let Some(view) = target.upgrade() {
                view.cover_check_pending.set(false);
            }
            if let Some(ui) = weak.upgrade() {
                ui.refresh_podcast_browser_covers();
            }
        });
    }

    pub(super) fn refresh_podcast_browser_covers(self: &Rc<Self>) {
        let Some(view) = self.podcast_browser.borrow().view.clone() else {
            return;
        };
        let targets: Vec<_> = view
            .rows
            .borrow()
            .iter()
            .filter_map(|(feed, row)| {
                let bounds = row.art.compute_bounds(&view.scroll)?;
                if !row.art.is_mapped()
                    || bounds.width() <= 0.
                    || bounds.height() <= 0.
                    || bounds.y() + bounds.height() <= 0.
                    || bounds.y() >= view.scroll.height() as f32
                {
                    return None;
                }
                let image = view.data.borrow().get(feed)?.show.image.clone()?;
                Some((feed.clone(), image, row.clone()))
            })
            .collect();
        for (feed, image, row) in targets {
            let path = media::podcast_cover_path(&image);
            if path.is_file() {
                crate::podcast_collection::set_cover(
                    &row.picture,
                    &row.art_path,
                    Some(path),
                    76 * self.win.scale_factor(),
                );
            } else {
                self.request_podcast_cover_url(&feed, &image);
            }
        }
    }

    pub(super) fn scan_podcast_list(self: &Rc<Self>, source: String) {
        if self.podcast_browser.borrow().scanning {
            return;
        }
        {
            let mut browse = self.podcast_browser.borrow_mut();
            browse.opened = true;
            browse.scanning = true;
            browse.message = format!(
                "Scanning {}…",
                if source == podcasts::RECOMMENDATIONS {
                    "Podcast Index recommendations"
                } else if source == podcasts::BBC {
                    "BBC feed list"
                } else {
                    &source
                }
            );
        }
        self.refresh_podcast_browser();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = podcasts::scan(&source).map(|catalog| (source, catalog));
            let _ = tx.send(Event::PodcastCatalog(result));
        });
    }

    pub(super) fn podcast_catalog_result(
        self: &Rc<Self>,
        result: Result<(String, podcasts::Catalog), String>,
    ) {
        let mut browse = self.podcast_browser.borrow_mut();
        browse.scanning = false;
        match result {
            Ok((source, catalog)) => {
                browse.revision += 1;
                browse.source = source;
                browse.shows = catalog.shows.into_iter().map(Rc::new).collect();
                browse.message = catalog.note;
                if catalog.skipped > 0 {
                    browse.message.push_str(&format!(
                        " {} duplicate or unsupported addresses skipped.",
                        catalog.skipped
                    ));
                }
                if catalog.truncated {
                    browse
                        .message
                        .push_str(" The source reached its configured show limit.");
                }
            }
            Err(error) => {
                browse.message = format!(
                    "Discovery failed: {error}. Existing results have been kept. Try Starter picks or another source."
                )
            }
        }
        drop(browse);
        if matches!(*self.page.borrow(), Page::Collection(ref s) if s == "podcasts/browse") {
            self.refresh_podcast_browser();
        }
    }

    fn preview_podcast(self: &Rc<Self>, value: String) {
        self.read_podcast_request(value, false, true);
    }
    pub(super) fn read_podcast(self: &Rc<Self>, value: String, subscribe: bool) {
        self.read_podcast_request(value, subscribe, false);
    }
    fn read_podcast_request(self: &Rc<Self>, value: String, subscribe: bool, preview: bool) {
        let feed = match podcasts::normalize_url(&value) {
            Ok(feed) => feed,
            Err(_) => {
                self.notice("Enter an HTTP or HTTPS feed URL without embedded credentials.");
                return;
            }
        };
        if preview {
            let mut browse = self.podcast_browser.borrow_mut();
            if !browse.shows.iter().any(|show| show.url == feed) {
                browse.revision += 1;
                browse.shows.insert(
                    0,
                    Rc::new(podcasts::Listing {
                        url: feed.clone(),
                        show: media::PodcastShow {
                            title: feed.clone(),
                            ..Default::default()
                        },
                    }),
                );
            }
            browse.query.clear();
        }
        if subscribe && podcasts::contains_feed(&self.db.borrow().feeds, &feed) {
            self.notice("You are already subscribed to this podcast.");
            return;
        }
        if self.podcast_jobs.borrow().contains(&feed) {
            if subscribe {
                self.podcast_subscriptions.borrow_mut().insert(feed.clone());
            }
            if preview {
                self.refresh_podcast_browser();
            }
            return;
        }
        if self.podcast_jobs.borrow().len() >= 4 {
            let mut queue = self.podcast_read_queue.borrow_mut();
            queue.retain(|(queued, _, _)| queued != &feed);
            if queue.len() == 32 {
                queue.pop_back();
            }
            queue.push_front((feed.clone(), subscribe, preview));
            drop(queue);
            self.refresh_podcast_views(&feed);
            self.notice("Podcast feed queued. It will load when a current read finishes.");
            return;
        }
        if subscribe {
            self.podcast_subscriptions.borrow_mut().insert(feed.clone());
        }
        self.podcast_browser.borrow_mut().feed_errors.remove(&feed);
        self.podcast_jobs.borrow_mut().insert(feed.clone());
        self.notice(if subscribe {
            "Reading feed before subscribing…"
        } else {
            "Reading podcast feed…"
        });
        if matches!(*self.page.borrow(), Page::Collection(ref s) if s == "podcasts/browse") {
            self.update_podcast_browser_feed(&feed);
        }
        self.refresh_podcast_views(&feed);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            if let Some(cached) = media::cached_podcast(&feed) {
                let _ = tx.send(Event::CachedPodcast(feed.clone(), cached));
            }
            let result = media::podcast(&feed, &media::podcast_cache_dir());
            let _ = tx.send(Event::Feed(feed, result));
        });
    }

    pub(super) fn podcast_feed_result(
        self: &Rc<Self>,
        feed: String,
        result: Result<media::PodcastFeed, String>,
    ) {
        self.podcast_jobs.borrow_mut().remove(&feed);
        let next = self.podcast_read_queue.borrow_mut().pop_front();
        if let Some((queued, subscribe, preview)) = next {
            self.read_podcast_request(queued, subscribe, preview);
        }
        let subscribe = self.podcast_subscriptions.borrow_mut().remove(&feed);
        match result {
            Ok(mut podcast) => {
                self.podcast_refreshed
                    .borrow_mut()
                    .insert(feed.clone(), Instant::now());
                if podcast.show.title.is_empty() {
                    podcast.show.title = feed.clone();
                }
                self.feeds
                    .borrow_mut()
                    .insert(feed.clone(), podcast.episodes);
                self.reorder_playing_podcast(&feed);
                let mut browse = self.podcast_browser.borrow_mut();
                browse.read.insert(feed.clone());
                if let Some(listing) = browse.shows.iter_mut().find(|s| s.url == feed) {
                    Rc::make_mut(listing).show = podcast.show.clone();
                }
                drop(browse);
                let mut db = self.db.borrow_mut();
                let already_subscribed = podcasts::contains_feed(&db.feeds, &feed);
                if subscribe && !already_subscribed {
                    db.feeds.push(feed.clone());
                }
                if subscribe || already_subscribed {
                    db.podcast_shows.insert(feed.clone(), podcast.show);
                    drop(db);
                    self.persist();
                }
                self.notice(if subscribe {
                    "Subscribed. Find this show in Your podcasts."
                } else {
                    "Feed read."
                });
            }
            Err(error) => {
                self.podcast_browser
                    .borrow_mut()
                    .feed_errors
                    .insert(feed.clone(), error.clone());
                self.notice(&error);
            }
        }
        self.refresh_podcast_views(&feed);
        let pending = self.podcast_latest_pending.borrow().as_deref() == Some(feed.as_str());
        if pending {
            self.podcast_latest_pending.borrow_mut().take();
            let latest = self
                .feeds
                .borrow()
                .get(&feed)
                .and_then(|items| items.first())
                .cloned();
            if let Some(latest) = latest {
                self.force_latest_podcast(&feed, &latest);
            }
        }
        self.finish_podcast_resume(&feed, true);
        match &*self.page.borrow() {
            Page::Collection(tab) if tab == "podcasts/browse" => {
                self.update_podcast_browser_feed(&feed)
            }
            Page::Collection(tab) if tab == "podcasts" => self.render(),
            _ => {}
        }
    }
    pub(super) fn show_asset(self: &Rc<Self>, p: &Path) {
        self.open_embedded(p, false);
    }
}

fn podcast_text(text: &str, class: &str) -> gtk::Label {
    let value = label(text, class);
    value.set_wrap(true);
    value.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    value.set_ellipsize(gtk::pango::EllipsizeMode::None);
    value.set_hexpand(true);
    value
}

fn podcast_button(ui: &Rc<Ui>, title: &str, callback: impl Fn(&Rc<Ui>) + 'static) -> gtk::Button {
    let ui = Rc::downgrade(ui);
    button(title, move || {
        if let Some(ui) = ui.upgrade() {
            callback(&ui);
        }
    })
}

fn podcast_row_bounds(
    row: &PodcastRow,
    scroll: &gtk::ScrolledWindow,
) -> Option<gtk::graphene::Rect> {
    if !row.root.is_mapped() {
        return None;
    }
    row.root.compute_bounds(scroll).filter(|bounds| {
        bounds.width() > 0.
            && bounds.height() > 0.
            && bounds.y() + bounds.height() > 0.
            && bounds.y() < scroll.height() as f32
    })
}

// GTK can revise its deletion anchor during layout. Correct once against the
// surviving row's actual viewport offset; any newer user input or model change
// cancels the correction instead of pulling the user back to an old location.
fn preserve_podcast_anchor(view: &Rc<PodcastBrowser>, url: String, offset: f32) {
    let activity = view.interaction.get();
    let generation = view.layout_generation.get();
    let allocated = Cell::new(false);
    let weak = Rc::downgrade(view);
    view.list.add_tick_callback(move |_, _| {
        let Some(view) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        if !view.root.is_mapped()
            || view.interaction.get() != activity
            || view.layout_generation.get() != generation
        {
            return glib::ControlFlow::Break;
        }
        // Tick callbacks precede allocation. The following frame sees the layout
        // produced by the deletion, including GTK's own focus/scroll adjustment.
        if !allocated.replace(true) {
            return glib::ControlFlow::Continue;
        }
        let anchor = view.rows.borrow().get(&url).cloned();
        if let Some(anchor) = anchor
            && let Some(bounds) = podcast_row_bounds(&anchor, &view.scroll)
        {
            let delta = bounds.y() - offset;
            if delta.abs() > 0.5 {
                let adjustment = view.scroll.vadjustment();
                adjustment.set_value(adjustment.value() + f64::from(delta));
            }
        }
        glib::ControlFlow::Break
    });
}
