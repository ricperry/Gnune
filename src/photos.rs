//! Virtualized photo browsing with cancellable filesystem work.
use super::*;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

struct Group {
    title: String,
    items: Arc<[PathBuf]>,
}
struct Catalog {
    groups: Vec<Group>,
    positions: Vec<(usize, usize)>,
}
fn catalog(paths: Vec<PathBuf>, by_date: bool, cancel: &AtomicBool) -> Option<Catalog> {
    let mut groups: BTreeMap<String, (String, Vec<PathBuf>)> = BTreeMap::new();
    for path in paths {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let (key, title) = if by_date {
            let date = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|m| glib::DateTime::from_unix_local(m.as_secs() as i64).ok());
            match date {
                Some(date) => (
                    date.format("%Y-%m").ok()?.to_string(),
                    date.format("%b %Y").ok()?.to_string(),
                ),
                None => ("Unknown date".into(), "Unknown date".into()),
            }
        } else {
            let parent = path.parent().unwrap_or_else(|| Path::new(""));
            (
                parent.to_string_lossy().into_owned(),
                parent
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            )
        };
        groups
            .entry(key)
            .or_insert_with(|| (title, Vec::new()))
            .1
            .push(path);
    }
    let mut catalog = Catalog {
        groups: Vec::new(),
        positions: Vec::new(),
    };
    for (_, (title, mut items)) in groups {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        items.sort();
        let group = catalog.groups.len();
        catalog
            .positions
            .extend((0..items.len()).map(|i| (group, i)));
        catalog.groups.push(Group {
            title,
            items: items.into(),
        });
    }
    Some(catalog)
}
fn tile(item: &gtk::ListItem) -> (gtk::Picture, gtk::Label, gtk::Label) {
    let content = item.child().unwrap().downcast::<gtk::Box>().unwrap();
    let group = content
        .first_child()
        .unwrap()
        .downcast::<gtk::Label>()
        .unwrap();
    let picture = group
        .next_sibling()
        .unwrap()
        .downcast::<gtk::Picture>()
        .unwrap();
    let title = picture
        .next_sibling()
        .unwrap()
        .downcast::<gtk::Label>()
        .unwrap();
    (picture, group, title)
}
impl Ui {
    pub(super) fn photos(self: &Rc<Self>, results: &gtk::Box) {
        let controls = row(6);
        let by_date = *self.filters.borrow() == "date";
        for (name, key) in [("Folders", ""), ("By date", "date")] {
            let button = app_button(self, name, move |ui| {
                *ui.filters.borrow_mut() = key.into();
                ui.render();
            });
            if (key == "date") == by_date {
                button.add_css_class("selected");
            }
            controls.append(&button);
        }
        results.append(&controls);
        let status = label("Reading pictures…", "subtitle");
        results.append(&status);
        let paths = self
            .db
            .borrow()
            .assets
            .iter()
            .filter(|p| media::is_photo_path(p))
            .cloned()
            .collect();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = cancelled.clone();
        results.connect_unrealize(move |_| stop.store(true, Ordering::Relaxed));
        let (tx, rx) = mpsc::channel();
        let stop = cancelled.clone();
        image_loader::background(cancelled.clone(), move || {
            if let Some(catalog) = catalog(paths, by_date, &stop) {
                let _ = tx.send(catalog);
            }
        });
        let weak = results.downgrade();
        let ui = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(16), move || {
            let Some(results) = weak.upgrade() else {
                cancelled.store(true, Ordering::Relaxed);
                return glib::ControlFlow::Break;
            };
            if cancelled.load(Ordering::Relaxed) {
                return glib::ControlFlow::Break;
            }
            let Ok(catalog) = rx.try_recv() else {
                return glib::ControlFlow::Continue;
            };
            let Some(ui) = ui.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if catalog.positions.is_empty() {
                status.set_text("Add a folder containing photos.");
                results.append(&app_button(&ui, "Add folder", |ui| ui.choose(true)));
            } else {
                status.set_text(&format!("{} pictures", catalog.positions.len()));
                ui.photo_grid(&results, Arc::new(catalog), cancelled.clone());
            }
            glib::ControlFlow::Break
        });
    }
    fn photo_grid(
        self: &Rc<Self>,
        results: &gtk::Box,
        catalog: Arc<Catalog>,
        cancelled: Arc<AtomicBool>,
    ) {
        let model = gtk::StringList::new(&[]);
        let selection = gtk::NoSelection::new(Some(model.clone()));
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, object| {
            let item = object.downcast_ref::<gtk::ListItem>().unwrap();
            let content = col(4);
            content.set_margin_start(8);
            content.set_margin_end(8);
            content.set_margin_top(8);
            content.set_margin_bottom(8);
            let group = label("", "eyebrow");
            group.set_max_width_chars(18);
            group.set_height_request(28);
            content.append(&group);
            let picture = gtk::Picture::new();
            picture.set_content_fit(gtk::ContentFit::Contain);
            picture.set_can_shrink(true);
            picture.set_size_request(120, 120);
            content.append(&picture);
            let title = label("", "track");
            title.set_max_width_chars(18);
            content.append(&title);
            item.set_child(Some(&content));
        });
        let photos = catalog.clone();
        factory.connect_bind(move |_, object| {
            let item = object.downcast_ref::<gtk::ListItem>().unwrap();
            let Some(position) = item
                .item()
                .and_downcast::<gtk::StringObject>()
                .and_then(|s| s.string().parse::<usize>().ok())
            else {
                return;
            };
            let (group_index, index) = photos.positions[position];
            let group = &photos.groups[group_index];
            let path = &group.items[index];
            let (picture, heading, title) = tile(item);
            heading.set_text(if index == 0 { &group.title } else { "" });
            heading.set_tooltip_text((index == 0).then_some(group.title.as_str()));
            title.set_text(&path.file_stem().unwrap_or_default().to_string_lossy());
            image_loader::load(&picture, path, 160);
        });
        factory.connect_unbind(|_, object| {
            let item = object.downcast_ref::<gtk::ListItem>().unwrap();
            let (picture, _, _) = tile(item);
            image_loader::cancel(&picture);
            picture.set_paintable(None::<&gtk::gdk::Texture>);
        });
        let grid = gtk::GridView::new(Some(selection), Some(factory));
        grid.add_css_class("photo-grid");
        grid.set_min_columns(1);
        grid.set_max_columns(12);
        grid.set_single_click_activate(true);
        let photos = catalog.clone();
        let ui = Rc::downgrade(self);
        grid.connect_activate(move |_, position| {
            if let Some(ui) = ui.upgrade() {
                let (group, index) = photos.positions[position as usize];
                ui.slides_shared(photos.groups[group].items.clone(), index);
            }
        });
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&grid));
        results.append(&scroll);
        // Populate the cheap model in bounded slices; GTK creates only viewport tiles.
        let mut position = 0;
        glib::idle_add_local(move || {
            if cancelled.load(Ordering::Relaxed) {
                return glib::ControlFlow::Break;
            }
            let end = (position + 256).min(catalog.positions.len());
            let values: Vec<_> = (position..end).map(|i| i.to_string()).collect();
            model.splice(
                model.n_items(),
                0,
                &values.iter().map(String::as_str).collect::<Vec<_>>(),
            );
            position = end;
            if position == catalog.positions.len() {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }
    pub(super) fn slides(&self, items: Vec<PathBuf>, index: usize) {
        self.slides_shared(items.into(), index);
    }
    fn slides_shared(&self, items: Arc<[PathBuf]>, index: usize) {
        if index >= items.len() {
            return;
        }
        let window = gtk::Window::builder()
            .transient_for(&self.win)
            .title("Photos")
            .default_width(820)
            .default_height(600)
            .build();
        let content = col(8);
        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_can_shrink(true);
        picture.set_hexpand(true);
        picture.set_vexpand(true);
        content.append(&picture);
        image_loader::load(&picture, &items[index], 1920);
        let controls = row(8);
        controls.set_halign(gtk::Align::Center);
        content.append(&controls);
        let current = Rc::new(Cell::new(index));
        let weak_picture = picture.downgrade();
        let step = Rc::new(move |delta: isize| {
            let next = (current.get() as isize + delta).rem_euclid(items.len() as isize) as usize;
            current.set(next);
            if let Some(picture) = weak_picture.upgrade() {
                image_loader::replace_when_ready(&picture, &items[next], 1920);
            }
        });
        for (name, delta) in [("Previous", -1), ("Next", 1)] {
            let step = step.clone();
            controls.append(&button(name, move || step(delta)));
        }
        let auto = gtk::ToggleButton::with_label("Slideshow");
        controls.append(&auto);
        let win = window.downgrade();
        controls.append(&button("Full screen", move || {
            if let Some(win) = win.upgrade() {
                if win.is_fullscreen() {
                    win.unfullscreen();
                } else {
                    win.fullscreen();
                }
            }
        }));
        let weak = window.downgrade();
        glib::timeout_add_seconds_local(5, move || {
            if weak.upgrade().is_none_or(|w| !w.is_visible()) {
                return glib::ControlFlow::Break;
            }
            if auto.is_active() {
                step(1);
            }
            glib::ControlFlow::Continue
        });
        let pic = picture.downgrade();
        window.connect_close_request(move |_| {
            if let Some(pic) = pic.upgrade() {
                image_loader::cancel(&pic);
            }
            glib::Propagation::Proceed
        });
        chrome::set_content(&window, &content);
        window.present();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grouping_preserves_original_paths_and_group_local_slide_positions() {
        let paths = vec![
            PathBuf::from("/one/b.jpg"),
            PathBuf::from("/two/a.jpg"),
            PathBuf::from("/one/a.jpg"),
        ];
        let catalog = catalog(paths, false, &AtomicBool::new(false)).unwrap();
        assert_eq!(catalog.groups.len(), 2);
        assert_eq!(catalog.positions, vec![(0, 0), (0, 1), (1, 0)]);
        assert_eq!(catalog.groups[0].items[0], Path::new("/one/a.jpg"));
    }
    #[test]
    fn missing_dates_remain_browsable_and_cancelled_work_stops() {
        let paths = vec![PathBuf::from("/missing/picture.jpg")];
        let catalog = catalog(paths.clone(), true, &AtomicBool::new(false)).unwrap();
        assert_eq!(catalog.groups[0].title, "Unknown date");
        assert!(super::catalog(paths, true, &AtomicBool::new(true)).is_none());
    }
}
