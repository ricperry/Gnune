use super::*;
use crate::recognition::{Candidate, Keys, Lookup, Method, Song};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) fn read_tags(tx: mpsc::Sender<Event>, paths: &[PathBuf]) {
    for batch in paths.chunks(16) {
        let tags: Vec<_> = batch
            .iter()
            .filter(|p| !media::is_photo_path(p))
            .filter_map(|p| recognition::probe(p).ok().map(|info| (p.clone(), info)))
            .collect();
        if !tags.is_empty() && tx.send(Event::VideoTags(tags)).is_err() {
            break;
        }
    }
}

impl Ui {
    pub(super) fn video_title(&self, path: &Path) -> String {
        let db = self.db.borrow();
        db.video_tags
            .get(path)
            .map(|v| v.song().title.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                db.episodes
                    .iter()
                    .find(|(_, p)| p.as_path() == path)
                    .map(|(s, _)| s.clone())
            })
            .unwrap_or_else(|| {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
    }

    pub(super) fn refresh_video_metadata(self: &Rc<Self>, changed: &[PathBuf]) {
        let gallery = self.videos.borrow().clone();
        if let Some(gallery) = gallery {
            gallery.refresh_metadata(self);
        }
        let mut media = self.media.borrow_mut();
        let Some(active) = media.as_mut().filter(|m| changed.contains(&m.path)) else {
            return;
        };
        active.title = self.video_title(&active.path);
        drop(media);
        self.update_cover();
        if *self.page.borrow() == Page::Video {
            self.render();
        }
    }

    pub(super) fn video_info(self: &Rc<Self>, path: &Path) {
        if self.db.borrow().video_tags.contains_key(path) {
            self.video_info_form(path);
            return;
        }
        let ui = self.clone();
        let path = path.to_owned();
        glib::spawn_future_local(async move {
            let p = path.clone();
            if let Ok(Ok(tags)) = gio::spawn_blocking(move || recognition::probe(&p)).await {
                ui.db.borrow_mut().merge_video_tags(path.clone(), tags);
            }
            ui.video_info_form(&path);
        });
    }

    fn video_info_form(self: &Rc<Self>, path: &Path) {
        let w = gtk::Window::builder()
            .title("Video song info")
            .transient_for(&self.win)
            .modal(true)
            .default_width(560)
            .default_height(610)
            .build();
        w.add_css_class("metadata-dialog");
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        let content = col(10);
        content.add_css_class("song-info");
        content.append(&label("song info", "heading"));
        let location = gtk::Expander::new(Some("File location"));
        let file = wrapping(&path.to_string_lossy());
        file.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        file.set_selectable(true);
        file.set_max_width_chars(52);
        location.set_child(Some(&file));
        content.append(&location);
        let fields = SongFields::new(&content);
        if let Some(tags) = self.db.borrow().video_tags.get(path) {
            fields.fill(tags.song());
        }
        let genre = gtk::Entry::new();
        genre.set_placeholder_text(Some("Unknown genre"));
        genre.set_text(
            self.db
                .borrow()
                .video_tags
                .get(path)
                .map_or("", |t| t.genre()),
        );
        field_row(&content, "Genre", &genre);
        let original_genre = genre.text().to_string();
        let original_song = fields.read();
        let selected = gtk::DropDown::from_strings(&[
            "My music collection",
            "AcoustID · whole recording",
            "AudD · audio passage",
        ]);
        selected.update_property(&[gtk::accessible::Property::Label("Identification method")]);
        let lookup = gtk::Button::with_label("Identify music");
        let row = row(8);
        row.append(&selected);
        row.append(&lookup);
        content.append(&row);
        let passage = gtk::SpinButton::with_range(0., 86400., 1.);
        passage.set_numeric(true);
        let start = if self.media.borrow().as_ref().is_some_and(|m| m.path == path) {
            let state = self.playback_state();
            state.position.min((state.duration - 10.).max(0.))
        } else {
            0.
        };
        passage.set_value(start);
        let timing = field_row(&content, "Start (seconds)", &passage);
        timing.set_visible(false);
        let about = wrapping("");
        content.append(&about);
        let mode_changed = {
            let about = about.clone();
            let timing = timing.clone();
            move |m: &gtk::DropDown| {
                let method = method(m);
                about.set_text(match method {
                    Method::Library => {
                        "Compare the soundtrack with music in your local collection."
                    }
                    Method::AcoustId => {
                        "Send the acoustic fingerprint and recording length to AcoustID."
                    }
                    Method::AudD => {
                        "Send up to 10 seconds of audio from the selected position to AudD."
                    }
                });
                timing.set_visible(method == Method::AudD);
            }
        };
        mode_changed(&selected);
        selected.connect_selected_notify(mode_changed);
        let keys = ServiceKeys::new(&content);
        let status = wrapping("");
        status.set_accessible_role(gtk::AccessibleRole::Status);
        content.append(&status);
        let matches = col(4);
        content.append(&matches);
        let save = gtk::Button::with_label("Save to collection");
        save.add_css_class("primary");
        save.set_halign(gtk::Align::Start);
        let ui = self.clone();
        let save_error = status.clone();
        let target = path.to_owned();
        let weak = w.downgrade();
        let form = fields.clone();
        save.connect_clicked(move |_| {
            let old = ui.db.borrow().video_tags.get(&target).cloned();
            {
                let mut db = ui.db.borrow_mut();
                let tags = db.video_tags.entry(target.clone()).or_default();
                let song = form.read();
                if song != original_song {
                    tags.saved = (song != tags.embedded).then_some(song);
                }
                let edited = genre.text();
                let edited = edited.trim();
                if edited != original_genre {
                    tags.saved_genre =
                        (Some(edited) != tags.genre.as_deref()).then(|| edited.to_owned());
                }
            }
            if ui.protect || ui.writer.save(&ui.db.borrow()).is_err() {
                let mut db = ui.db.borrow_mut();
                if let Some(old) = old {
                    db.video_tags.insert(target.clone(), old);
                } else {
                    db.video_tags.remove(&target);
                }
                save_error.set_text("Could not save song info to the library.");
                return;
            }
            if let Some(w) = weak.upgrade() {
                w.close();
            }
            ui.refresh_video_metadata(std::slice::from_ref(&target));
        });
        content.append(&save);
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        w.connect_close_request(move |_| {
            token.store(true, Ordering::Relaxed);
            glib::Propagation::Proceed
        });
        let ui = Rc::downgrade(self);
        let path = path.to_owned();
        let status_on_start = status.clone();
        let win = w.downgrade();
        lookup.connect_clicked(move |button| {
            let Some(ui) = ui.upgrade() else { return };
            let chosen = method(&selected);
            let token = keys.current().token(chosen);
            if chosen == Method::AudD && token.is_empty() {
                keys.expander.set_expanded(true);
                status_on_start.set_text("Enter an AudD API token to identify a passage.");
                return;
            }

            let mut tracks = Vec::new();
            if chosen == Method::Library {
                let db = ui.db.borrow();
                if db.tracks.is_empty() {
                    status_on_start
                        .set_text("Add a tagged music folder, or choose an online service.");
                    return;
                }
                let duration = db
                    .video_tags
                    .get(&path)
                    .map(|v| v.duration)
                    .unwrap_or_default();
                tracks.extend(
                    db.tracks
                        .iter()
                        .filter(|t| {
                            duration <= 0. || recognition::similar_duration(t.duration, duration)
                        })
                        .cloned(),
                );
            }
            let request = Lookup {
                method: chosen,
                source: path.clone(),
                start: passage.value(),
                token,
                tracks,
            };
            let cancelled = cancel.clone();
            let status = status_on_start.clone();
            let list = matches.clone();
            let form = fields.clone();
            let button = button.clone();
            let weak = win.clone();
            let picker = selected.clone();
            let position = passage.clone();

            button.set_sensitive(false);
            picker.set_sensitive(false);
            position.set_sensitive(false);
            clear(&list);
            status.set_text("Identifying the soundtrack…");
            glib::spawn_future_local(async move {
                let result = gio::spawn_blocking(move || {
                    request.run(&data_dir().join("fingerprints"), &cancelled)
                })
                .await
                .unwrap_or_else(|_| Err("Identification stopped unexpectedly.".into()));
                if weak.upgrade().is_none_or(|w| !w.is_visible()) {
                    return;
                }
                button.set_sensitive(true);
                picker.set_sensitive(true);
                position.set_sensitive(true);
                match result {
                    Err(message) => status.set_text(&message),
                    Ok(choices) if choices.is_empty() => status
                        .set_text("No match found. Try another method or enter the song info."),
                    Ok(choices) => {
                        status.set_text("Choose a match, review the fields, then save.");
                        show_matches(&list, &form, choices);
                    }
                }
            });
        });
        scroll.set_child(Some(&content));
        chrome::set_content(&w, &scroll);
        w.present();
    }
}

fn method(picker: &gtk::DropDown) -> Method {
    match picker.selected() {
        1 => Method::AcoustId,
        2 => Method::AudD,
        _ => Method::Library,
    }
}
fn wrapping(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_wrap(true);
    l.set_xalign(0.);
    l.add_css_class("small");
    l
}
fn field_row<W: IsA<gtk::Widget>>(parent: &gtk::Box, name: &str, field: &W) -> gtk::Box {
    let r = row(10);
    let l = label(name, "small");
    l.set_width_chars(14);
    l.set_mnemonic_widget(Some(field));
    field
        .as_ref()
        .update_property(&[gtk::accessible::Property::Label(name)]);
    field.as_ref().set_hexpand(true);
    r.append(&l);
    r.append(field);
    parent.append(&r);
    r
}
#[derive(Clone)]
struct SongFields {
    title: gtk::Entry,
    artist: gtk::Entry,
    album: gtk::Entry,
}
impl SongFields {
    fn new(parent: &gtk::Box) -> Self {
        let f = Self {
            title: gtk::Entry::new(),
            artist: gtk::Entry::new(),
            album: gtk::Entry::new(),
        };
        for (name, entry) in [
            ("Song title", &f.title),
            ("Artist", &f.artist),
            ("Album", &f.album),
        ] {
            field_row(parent, name, entry);
        }
        f
    }
    fn fill(&self, s: &Song) {
        self.title.set_text(&s.title);
        self.artist.set_text(&s.artist);
        self.album.set_text(&s.album);
    }
    fn read(&self) -> Song {
        Song {
            title: self.title.text().trim().into(),
            artist: self.artist.text().trim().into(),
            album: self.album.text().trim().into(),
        }
    }
}

struct ServiceKeys {
    expander: gtk::Expander,
    acoustid: gtk::PasswordEntry,
    audd: gtk::PasswordEntry,
}
impl ServiceKeys {
    fn new(parent: &gtk::Box) -> Self {
        let keys = Keys::load();
        let expander = gtk::Expander::new(Some("Recognition service keys"));
        let body = col(5);
        let acoustid = gtk::PasswordEntry::new();
        let audd = gtk::PasswordEntry::new();
        body.append(&wrapping(
            "AcoustID is ready to use with Gnune’s registered application key. An override is optional. AudD requires your own token.",
        ));
        field_row(&body, "AcoustID override (optional)", &acoustid);
        field_row(&body, "AudD token", &audd);
        let links = row(8);
        links.append(&gtk::LinkButton::with_label(
            "https://dashboard.audd.io/",
            "Get an AudD token",
        ));
        body.append(&links);
        let result = wrapping("");
        let save = gtk::Button::with_label("Remember keys on this computer");
        let a = acoustid.clone();
        let d = audd.clone();
        let feedback = result.clone();
        match keys {
            Ok(keys) => {
                acoustid.set_text(&keys.acoustid);
                audd.set_text(&keys.audd);
            }
            Err(e) => result.set_text(&e),
        }
        save.connect_clicked(move |_| {
            let keys = Keys {
                acoustid: a.text().trim().into(),
                audd: d.text().trim().into(),
            };
            match keys.save() {
                Ok(()) => feedback.set_text("Keys saved."),
                Err(e) => feedback.set_text(&e),
            }
        });
        body.append(&save);
        body.append(&result);
        expander.set_child(Some(&body));
        parent.append(&expander);
        Self {
            expander,
            acoustid,
            audd,
        }
    }
    fn current(&self) -> Keys {
        Keys {
            acoustid: self.acoustid.text().trim().into(),
            audd: self.audd.text().trim().into(),
        }
    }
}

fn show_matches(list: &gtk::Box, form: &SongFields, choices: Vec<Candidate>) {
    let names: Vec<String> = choices
        .iter()
        .map(|c| {
            let mut title = format!("{} · {}", c.song.artist, c.song.title);
            if !c.song.album.is_empty() {
                title.push_str(&format!(" · {}", c.song.album));
            }
            if let Some(n) = c.confidence {
                title.push_str(&format!(" ({:.1}% match)", n * 100.));
            }
            title
        })
        .collect();
    let picker = gtk::DropDown::from_strings(&names.iter().map(String::as_str).collect::<Vec<_>>());
    picker.update_property(&[gtk::accessible::Property::Label(
        "Recording and album matches",
    )]);
    picker.set_enable_search(true);
    list.append(&picker);
    let use_match = gtk::Button::with_label("Use selected match");
    let form = form.clone();
    use_match.connect_clicked(move |_| {
        if let Some(c) = choices.get(picker.selected() as usize) {
            for (field, value) in [
                (&form.title, &c.song.title),
                (&form.artist, &c.song.artist),
                (&form.album, &c.song.album),
            ] {
                if !value.is_empty() {
                    field.set_text(value);
                }
            }
        }
    });
    list.append(&use_match);
}
