mod chrome;
mod collection_media;
mod embedded;
mod enrichment;
mod image_loader;
mod media;
mod motion;
mod mpris;
mod photos;
mod playhead;
mod recognition;
mod screens;
mod showcase;
mod state;
mod sync;
mod theme;
mod video_info;
mod visual;
use gnune::{
    library::{Track, scan_paths},
    player::Player,
};
use gtk::{gio, glib, prelude::*};
use rand::seq::SliceRandom;
use state::{Library, album_key, data_dir};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};
#[derive(Clone, PartialEq)]
enum Page {
    Now,
    Mix(String),
    Quick,
    Collection(String),
    Album(String),
    Artist(String),
    Playlist(String),
    Queue,
    Lyrics,
    Wall,
    Showcase,
    Devices,
    Video,
}
enum Event {
    SaveError(String),
    PosterChanged(PathBuf),
    VideoTags(Vec<(PathBuf, recognition::VideoTags)>),
    Scan(Vec<Track>, Vec<PathBuf>, Vec<String>, bool),
    Details(PathBuf, enrichment::Details),
    Feed(String, Result<Vec<media::Episode>, String>),
    Podcast(String, Result<PathBuf, String>),
    Progress(PathBuf, f64),
}
#[derive(PartialEq)]
enum Surface {
    Artwork(Option<PathBuf>),
    Portrait {
        image: PathBuf,
        art: Option<PathBuf>,
    },
}
#[derive(Clone, Copy)]
struct FullPlayerWindow {
    size: (i32, i32),
    maximized: bool,
    fullscreen: bool,
}
struct Ui {
    win: gtk::ApplicationWindow,
    nav: gtk::Box,
    back: gtk::Button,
    history: RefCell<Vec<Page>>,
    browser: RefCell<std::rc::Weak<screens::Browser>>,
    videos: RefCell<Option<screens::VideoGallery>>,
    poster_jobs: mpsc::Sender<Vec<PathBuf>>,
    surface: gtk::Stack,
    background: RefCell<Option<Rc<Surface>>>,
    body: gtk::Box,
    motion: motion::Stage,
    rendered_page: RefCell<Option<screens::ViewState>>,
    bar: gtk::Box,
    status: gtk::Label,
    play: gtk::Button,
    next: gtk::Button,
    elapsed: gtk::Label,
    remaining: gtk::Label,
    seek: gtk::Scale,
    mini_title: gtk::Label,
    thumb: gtk::Picture,
    thumb_art: RefCell<Option<PathBuf>>,
    db: RefCell<Library>,
    writer: state::LibraryWriter,
    player: Player,
    media: RefCell<Option<embedded::Media>>,
    page: RefCell<Page>,
    queue: RefCell<Vec<Track>>,
    current: RefCell<Option<Track>>,
    tx: mpsc::Sender<Event>,
    details: RefCell<HashMap<PathBuf, enrichment::Details>>,
    requested: RefCell<std::collections::HashSet<PathBuf>>,
    pos: Cell<f64>,
    duration: Cell<f64>,
    shuffle: Cell<bool>,
    repeat: Cell<bool>,
    pending_seek: Cell<Option<f64>>,
    playhead: RefCell<playhead::Playhead>,
    playhead_tick: RefCell<Option<gtk::TickCallbackId>>,
    sequence: RefCell<Vec<Track>>,
    last_input: Cell<Instant>,
    mini: Cell<bool>,
    full_player_window: Cell<Option<FullPlayerWindow>>,
    fitting_mini: Cell<bool>,
    restoring_window: Cell<Option<FullPlayerWindow>>,
    window_transition_started: Cell<Instant>,
    mini_action: gtk::Button,
    fullscreen_action: gtk::Button,
    compact: Cell<bool>,
    follow: Cell<bool>,
    lyrics: RefCell<Vec<(f64, gtk::Button)>>,
    lyric_scroll: RefCell<Option<gtk::ScrolledWindow>>,
    active_line: Cell<Option<usize>>,
    theme: Rc<theme::Theme>,
    filters: RefCell<String>,
    last_error: RefCell<Option<String>>,
    server: RefCell<Option<sync::Session>>,
    feeds: RefCell<HashMap<String, Vec<media::Episode>>>,
    busy_enrich: Cell<bool>,
    protect: bool,
}
fn label(s: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(s));
    l.set_xalign(0.);
    l.add_css_class(class);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l
}
fn button(s: &str, act: impl Fn() + 'static) -> gtk::Button {
    let b = gtk::Button::with_label(s);
    b.connect_clicked(move |_| act());
    b
}
fn icon(name: &str, tip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(name);
    b.set_tooltip_text(Some(tip));
    b.set_accessible_role(gtk::AccessibleRole::Button);
    b
}
fn gap() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    b.set_hexpand(true);
    b
}
fn row(n: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, n)
}
fn col(n: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, n)
}
fn clear(b: &gtk::Box) {
    while let Some(w) = b.first_child() {
        b.remove(&w);
    }
}
fn time(s: f64) -> String {
    let n = s.max(0.) as u64;
    format!("{}:{:02}", n / 60, n % 60)
}
fn app_button(u: &Rc<Ui>, s: &str, f: impl Fn(&Rc<Ui>) + 'static) -> gtk::Button {
    let v = u.clone();
    button(s, move || f(&v))
}
fn action(u: &Rc<Ui>, b: &gtk::Button, f: impl Fn(&Rc<Ui>) + 'static) {
    let v = u.clone();
    b.connect_clicked(move |_| f(&v));
}
fn main() -> glib::ExitCode {
    gio::resources_register_include!("gnune.gresource").expect("embedded Gnune resources");
    glib::set_application_name("Gnune");
    let app = gtk::Application::builder()
        .application_id("org.gnune.Player")
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    let slot: Rc<RefCell<Option<Rc<Ui>>>> = Default::default();
    let s = slot.clone();
    app.connect_activate(move |a| {
        if let Some(u) = s.borrow().as_ref() {
            u.win.present();
            return;
        }
        *s.borrow_mut() = build(a);
    });
    app.connect_open(move |a, files, _| {
        a.activate();
        if let Some(u) = slot.borrow().as_ref() {
            u.import(files.iter().filter_map(|f| f.path()).collect(), true);
        }
    });
    app.run()
}
fn build(app: &gtk::Application) -> Option<Rc<Ui>> {
    let win = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Gnune")
        .default_width(1100)
        .default_height(720)
        .build();
    let base = gtk::CssProvider::new();
    base.load_from_string(include_str!("style.css"));
    let display = gtk::prelude::WidgetExt::display(&win);
    gtk::style_context_add_provider_for_display(&display, &base, 600);
    gtk::IconTheme::for_display(&display).add_resource_path("/org/gnune/Player/icons");
    gtk::Window::set_default_icon_name("org.gnune.Player");
    win.set_icon_name(Some("org.gnune.Player"));
    let theme = theme::Theme::new(&display);
    let (db, problem) = match state::load() {
        Ok(db) => (db, None),
        Err(e) => (Library::default(), Some(e)),
    };
    let player = match Player::new() {
        Ok(p) => p,
        Err(e) => {
            let l = label(&format!("Unable to start playback engine: {e}"), "subtitle");
            chrome::set_content(&win, &l);
            win.present();
            return None;
        }
    };
    let root = col(0);
    let canvas = gtk::Overlay::new();
    let surface = gtk::Stack::new();
    surface.set_transition_type(gtk::StackTransitionType::Crossfade);
    surface.set_transition_duration(theme::FADE_MS);
    surface.set_hhomogeneous(false);
    surface.set_vhomogeneous(false);
    surface.connect_transition_running_notify(visual::discard_old_backdrops);
    surface.set_hexpand(true);
    surface.set_vexpand(true);
    surface.set_can_target(false);
    canvas.set_child(Some(&surface));
    canvas.add_overlay(&root);
    canvas.set_measure_overlay(&root, true);
    chrome::set_content(&win, &canvas);
    let nav = row(8);
    nav.add_css_class("nav");
    root.append(&nav);
    let body = col(0);
    body.set_vexpand(true);
    let motion = motion::Stage::new(&body);
    root.append(&motion);
    let status = label("", "status");
    status.set_visible(false);
    root.append(&status);
    let bar = row(8);
    bar.add_css_class("transport");
    root.append(&bar);
    let devices = icon("phone-symbolic", "Devices");
    let queue = icon("view-list-symbolic", "Up next");
    let shortcuts = row(2);
    shortcuts.add_css_class("shortcuts");
    shortcuts.append(&devices);
    shortcuts.append(&queue);
    bar.append(&shortcuts);
    bar.append(&gap());
    let thumb = gtk::Picture::new();
    thumb.set_content_fit(gtk::ContentFit::Contain);
    thumb.set_can_shrink(true);
    // Keep the transport's footprint independent of the video sink's native resolution.
    let preview = gtk::Overlay::new();
    preview.set_size_request(52, 38);
    preview.set_valign(gtk::Align::Center);
    preview.add_overlay(&thumb);
    preview.set_measure_overlay(&thumb, false);
    let listening = gtk::Button::new();
    listening.set_tooltip_text(Some("Now playing"));
    listening.set_child(Some(&preview));
    bar.append(&listening);
    let position = col(0);
    position.set_valign(gtk::Align::Center);
    position.set_hexpand(false);
    position.set_width_request(170);
    let mini_title = label("Nothing playing", "now-title");
    mini_title.set_max_width_chars(28);
    position.append(&mini_title);
    let seek = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 1., 0.1);
    seek.set_hexpand(true);
    seek.set_draw_value(false);
    seek.set_tooltip_text(Some("Playback position"));
    seek.set_sensitive(false);
    position.append(&seek);
    bar.append(&position);
    let elapsed = label("0:00", "clock");
    let remaining = label("0:00", "clock");
    for clock in [&elapsed, &remaining] {
        clock.set_ellipsize(gtk::pango::EllipsizeMode::None);
    }
    elapsed.set_tooltip_text(Some("Elapsed"));
    remaining.set_tooltip_text(Some("Remaining"));
    let times = col(0);
    times.set_valign(gtk::Align::Center);
    times.append(&elapsed);
    times.append(&remaining);
    bar.append(&times);
    bar.append(&gap());
    let prev = icon("media-skip-backward-symbolic", "Previous track");
    let play = icon("media-playback-start-symbolic", "Play or pause");
    let next = icon("media-skip-forward-symbolic", "Next track");
    let transport_buttons = row(4);
    transport_buttons.set_valign(gtk::Align::Center);
    for b in [&prev, &play, &next] {
        b.add_css_class("round");
        b.set_valign(gtk::Align::Center);
        transport_buttons.append(b);
    }
    play.add_css_class("play");
    bar.append(&transport_buttons);
    let volume = gtk::Scale::with_range(gtk::Orientation::Vertical, 0., 1., 0.01);
    volume.set_inverted(true);
    volume.set_draw_value(false);
    volume.set_height_request(110);
    volume.set_tooltip_text(Some("Volume"));
    let level = db.volume.unwrap_or(80.);
    volume.set_value(level / 100.);
    let _ = player.set_volume(level);
    let volume_menu = gtk::MenuButton::builder()
        .icon_name("audio-volume-high-symbolic")
        .tooltip_text("Volume")
        .build();
    let volume_pop = gtk::Popover::new();
    volume_pop.set_child(Some(&volume));
    volume_menu.set_popover(Some(&volume_pop));
    bar.append(&volume_menu);
    let more = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .tooltip_text("Playback options")
        .build();
    bar.append(&more);
    let back = icon("go-previous-symbolic", "Go back (Alt+Left)");
    back.set_valign(gtk::Align::Center);
    back.add_css_class("back");
    back.set_sensitive(false);
    let (tx, rx) = mpsc::channel();
    let (poster_jobs, jobs) = mpsc::channel::<Vec<PathBuf>>();
    let notify = tx.clone();
    std::thread::spawn(move || {
        let dir = data_dir().join("posters");
        for paths in jobs {
            for path in paths {
                if matches!(media::prepare_poster(&path, &dir), Ok(true))
                    && notify.send(Event::PosterChanged(path)).is_err()
                {
                    return;
                }
            }
        }
    });
    let protect = problem.is_some();
    let first_page = if db.tracks.is_empty() {
        Page::Collection("artists".into())
    } else {
        Page::Quick
    };
    let write_errors = tx.clone();
    let writer = state::LibraryWriter::new(data_dir(), move |e| {
        let _ = write_errors.send(Event::SaveError(e.to_string()));
    });
    let u = Rc::new(Ui {
        win,
        nav,
        back,
        surface,
        history: Default::default(),
        browser: Default::default(),
        videos: Default::default(),
        poster_jobs,
        background: Default::default(),
        body,
        motion,
        rendered_page: Default::default(),
        bar,
        status,
        play,
        next: next.clone(),
        elapsed,
        remaining,
        seek,
        mini_title,
        thumb,
        thumb_art: Default::default(),
        db: RefCell::new(db),
        writer,
        player,
        media: Default::default(),
        page: RefCell::new(first_page),
        queue: Default::default(),
        current: Default::default(),
        tx,
        details: Default::default(),
        requested: Default::default(),
        pos: Cell::new(0.),
        duration: Cell::new(0.),
        shuffle: Cell::new(false),
        repeat: Cell::new(false),
        pending_seek: Cell::new(None),
        playhead: Default::default(),
        playhead_tick: Default::default(),
        sequence: Default::default(),
        last_input: Cell::new(Instant::now()),
        mini: Cell::new(false),
        full_player_window: Cell::new(None),
        fitting_mini: Cell::new(false),
        restoring_window: Cell::new(None),
        window_transition_started: Cell::new(Instant::now()),
        mini_action: gtk::Button::with_label("Mini player"),
        fullscreen_action: gtk::Button::with_label("Full screen"),
        compact: Cell::new(false),
        follow: Cell::new(true),
        lyrics: Default::default(),
        lyric_scroll: Default::default(),
        active_line: Cell::new(None),
        theme,
        filters: Default::default(),
        last_error: Default::default(),
        server: Default::default(),
        feeds: Default::default(),
        busy_enrich: Cell::new(false),
        protect,
    });
    let v = u.clone();
    volume.connect_value_changed(move |scale| {
        let n = scale.value();
        let _ = v.set_playback_volume(n * 100.);
        v.db.borrow_mut().volume = Some(n * 100.);
    });
    action(&u, &prev, |v| {
        if v.pos.get() > 3. {
            v.command(v.seek_playback(0.));
        } else {
            v.command(v.previous_playback());
        }
    });
    action(&u, &next, |v| v.command(v.next_playback()));
    action(&u, &u.play, |v| v.toggle());
    let v = u.clone();
    u.seek.connect_change_value(move |_, _, n| {
        v.command(v.seek_playback(n));
        glib::Propagation::Proceed
    });
    action(&u, &devices, |u| u.navigate(Page::Devices));
    action(&u, &queue, |u| u.navigate(Page::Queue));
    action(&u, &listening, |u| u.navigate(Page::Now));
    action(&u, &u.back, |u| u.go_back());
    u.nav.append(&u.back);
    for (s, p) in [
        ("quickplay", Page::Quick),
        ("collection", Page::Collection("artists".into())),
        ("device", Page::Devices),
        ("now playing", Page::Now),
    ] {
        let link = app_button(&u, s, move |u| u.go(p.clone()));
        link.add_css_class("pivot");
        u.nav.append(&link);
    }
    u.nav.append(&gap());
    let add = icon("list-add-symbolic", "Add media folder");
    let options = gtk::MenuButton::builder()
        .label("settings")
        .tooltip_text("Library and settings")
        .build();
    options.add_css_class("settings");
    action(&u, &add, |u| u.choose(true));
    u.nav.append(&add);
    u.nav.append(&options);
    let brand = gtk::Image::from_icon_name("org.gnune.Player");
    brand.set_pixel_size(28);
    brand.set_tooltip_text(Some("Gnune (pronounced noon)"));
    brand.add_css_class("brand");
    u.nav.append(&brand);
    let menu = col(4);
    let pop = gtk::Popover::new();
    pop.set_child(Some(&menu));
    options.set_popover(Some(&pop));
    for (s, p) in [("Devices", Page::Devices), ("Queue", Page::Queue)] {
        let pop = pop.clone();
        menu.append(&app_button(&u, s, move |u| {
            pop.popdown();
            u.go(p.clone())
        }));
    }
    for (s, folder) in [("Open file", false), ("Add folder", true)] {
        let pop = pop.clone();
        menu.append(&app_button(&u, s, move |u| {
            pop.popdown();
            u.choose(folder)
        }));
    }
    let online = gtk::CheckButton::with_label("Look up missing art, lyrics and artist links");
    online.set_active(u.db.borrow().enrichment);
    let v = u.clone();
    online.connect_toggled(move |b| {
        v.db.borrow_mut().enrichment = b.is_active();
        v.persist();
        if b.is_active() {
            v.enrich();
        }
    });
    menu.append(&online);
    let opts = col(3);
    let pop = gtk::Popover::new();
    pop.set_child(Some(&opts));
    more.set_popover(Some(&pop));
    for (s, p) in [
        ("Up next", Page::Queue),
        ("Lyrics", Page::Lyrics),
        ("Current album", Page::Now),
        ("Collection art wall", Page::Wall),
        ("Artist showcase", Page::Showcase),
    ] {
        let pop = pop.clone();
        opts.append(&app_button(&u, s, move |v| {
            pop.popdown();
            v.navigate(p.clone())
        }));
    }
    let stop_pop = pop.clone();
    opts.append(&app_button(&u, "Stop", move |u| {
        stop_pop.popdown();
        u.command(u.stop_playback());
    }));
    for (s, shuf) in [("Shuffle queue", true), ("Repeat queue", false)] {
        let b = gtk::CheckButton::with_label(s);
        let v = u.clone();
        b.connect_toggled(move |b| {
            if shuf {
                v.shuffle.set(b.is_active());
                v.reshuffle();
            } else {
                v.repeat.set(b.is_active())
            }
        });
        opts.append(&b);
    }
    let full_pop = pop.clone();
    action(&u, &u.fullscreen_action, move |v| {
        full_pop.popdown();
        v.fullscreen();
    });
    opts.append(&u.fullscreen_action);
    action(&u, &u.mini_action, move |v| {
        pop.popdown();
        v.toggle_mini();
    });
    opts.append(&u.mini_action);
    for name in ["share-queue", "copy-pairing-link"] {
        let a = gio::SimpleAction::new(name, None);
        let v = u.clone();
        a.connect_activate(move |_, _| {
            if name == "share-queue" {
                v.share_queue()
            } else {
                v.copy_pairing_link()
            }
        });
        app.add_action(&a);
    }
    let resume = gio::SimpleAction::new("resume-from-phone", Some(glib::VariantTy::STRING));
    let v = u.clone();
    resume.connect_activate(move |_, value| {
        if let Some(id) = value.and_then(|s| s.str()) {
            let selected = {
                let db = v.db.borrow();
                db.tracks
                    .iter()
                    .find(|t| t.id == id)
                    .and_then(|t| db.phone_progress.get(&t.path).map(|pos| (t.clone(), *pos)))
            };
            if let Some((track, pos)) = selected {
                v.start(vec![track], 0, Some(pos));
                v.go(Page::Now);
            }
        }
    });
    app.add_action(&resume);
    let fullscreen = gio::SimpleAction::new("fullscreen", None);
    let v = u.clone();
    fullscreen.connect_activate(move |_, _| v.fullscreen());
    app.add_action(&fullscreen);
    app.set_accels_for_action("app.fullscreen", &["F11", "<Alt>Return"]);
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let v = u.clone();
    keys.connect_key_pressed(move |_, key, _, mods| {
        v.wake();
        if key == gtk::gdk::Key::Left && mods.contains(gtk::gdk::ModifierType::ALT_MASK) {
            v.go_back();
            return glib::Propagation::Stop;
        }
        if *v.page.borrow() == Page::Video && mods.is_empty() {
            let handled = match key {
                gtk::gdk::Key::space => {
                    v.toggle();
                    true
                }
                gtk::gdk::Key::Left => {
                    v.command(v.seek_playback((v.pos.get() - 10.).max(0.)));
                    true
                }
                gtk::gdk::Key::Right => {
                    v.command(v.seek_playback((v.pos.get() + 10.).min(v.duration.get())));
                    true
                }
                gtk::gdk::Key::bracketleft => {
                    v.command(v.set_playback_rate((v.playback_rate() - 0.25).max(0.25)));
                    true
                }
                gtk::gdk::Key::bracketright => {
                    v.command(v.set_playback_rate((v.playback_rate() + 0.25).min(4.)));
                    true
                }
                gtk::gdk::Key::j => {
                    if let Some(media) = v.media.borrow().as_ref() {
                        v.notice(&media.cycle_subtitles());
                    }
                    true
                }
                gtk::gdk::Key::f => {
                    v.fullscreen();
                    true
                }
                _ => false,
            };
            if handled {
                return glib::Propagation::Stop;
            }
        }
        if mods.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
            let page = match key {
                gtk::gdk::Key::l => Some(Page::Lyrics),
                gtk::gdk::Key::j => Some(Page::Queue),
                gtk::gdk::Key::_1 => Some(Page::Quick),
                gtk::gdk::Key::_2 => Some(Page::Collection("artists".into())),
                gtk::gdk::Key::_3 => Some(Page::Now),
                _ => None,
            };
            if let Some(p) = page {
                v.navigate(p);
                return glib::Propagation::Stop;
            }
        }
        if key == gtk::gdk::Key::Escape && v.win.is_fullscreen() {
            v.win.unfullscreen();
            return glib::Propagation::Stop;
        }
        if key == gtk::gdk::Key::o && mods.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
            v.choose(false);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    u.win.add_controller(keys);
    let motion = gtk::EventControllerMotion::new();
    let v = u.clone();
    motion.connect_motion(move |_, _, _| v.wake());
    u.win.add_controller(motion);
    let v = Rc::downgrade(&u);
    u.win.connect_unmap(move |_| {
        if let Some(v) = v.upgrade()
            && let Some(tick) = v.playhead_tick.take()
        {
            tick.remove();
        }
    });
    let v = u.clone();
    u.win.connect_close_request(move |_| {
        v.save_position();
        v.persist();
        v.close_embedded();
        v.player.shutdown();
        if let Some(session) = v.server.borrow_mut().take() {
            std::thread::spawn(move || drop(session));
        }
        glib::Propagation::Proceed
    });
    let v = u.clone();
    glib::timeout_add_local(Duration::from_millis(160), move || {
        while let Ok(e) = rx.try_recv() {
            v.event(e);
        }
        v.tick();
        glib::ControlFlow::Continue
    });
    let v = u.clone();
    u.win.connect_fullscreened_notify(move |_| {
        v.window_mode_changed();
        v.fullscreen_action.set_label(if v.win.is_fullscreen() {
            "Leave full screen"
        } else {
            "Full screen"
        });
        v.wake();
        v.render();
    });
    let v = u.clone();
    u.win
        .connect_maximized_notify(move |_| v.window_mode_changed());
    mpris::register(&u);
    u.render();
    u.win.present();
    u.queue_posters(&u.db.borrow().assets);
    let missing: Vec<_> = {
        let db = u.db.borrow();
        db.assets
            .iter()
            .filter(|p| {
                !media::is_photo_path(p)
                    && db
                        .video_tags
                        .get(*p)
                        .is_none_or(|tags| tags.genre.is_none())
            })
            .cloned()
            .collect()
    };
    if !missing.is_empty() {
        let tx = u.tx.clone();
        std::thread::spawn(move || video_info::read_tags(tx, &missing));
    }
    if let Some(e) = problem {
        u.notice(&format!(
            "{e}. Collection saving disabled to preserve the existing file."
        ));
    }
    Some(u)
}
impl Ui {
    fn persist(&self) {
        if !self.protect
            && let Err(e) = self.writer.save(&self.db.borrow())
        {
            self.notice(&e.to_string());
        }
    }
    fn persist_playback(&self) {
        if !self.protect
            && let Err(e) = self.writer.playback_changed(&self.db.borrow())
        {
            self.notice(&e.to_string());
        }
    }
    fn notice(&self, s: &str) {
        if self.status.text() != s {
            self.status.set_text(s);
        }
        self.status.set_tooltip_text(Some(s));
        self.status.set_visible(!s.is_empty());
    }
    fn command(&self, r: Result<(), String>) {
        if let Err(e) = r {
            self.notice(&e);
        }
    }
    fn playback_page(&self, page: Page) -> Page {
        if page == Page::Now && self.media.borrow().is_some() {
            Page::Video
        } else {
            page
        }
    }
    fn go(self: &Rc<Self>, p: Page) {
        let p = self.playback_page(p);
        if p == Page::Video && self.mini.get() {
            self.toggle_mini();
        }
        if p != *self.page.borrow() {
            let previous = self.page.replace(p);
            self.history.borrow_mut().push(previous);
            self.back.set_sensitive(!self.history.borrow().is_empty());
        }
        self.filters.borrow_mut().clear();
        self.render();
    }
    // Explicit navigation reveals its destination; playback refreshes can keep
    // using go/render without expanding the mini player.
    fn navigate(self: &Rc<Self>, p: Page) {
        if self.mini.get() {
            self.toggle_mini();
        }
        self.go(p);
    }
    fn go_back(self: &Rc<Self>) {
        if self.mini.get() {
            self.toggle_mini();
        }
        let mut history = self.history.borrow_mut();
        while self.media.borrow().is_none() && history.last() == Some(&Page::Video) {
            history.pop();
        }
        if let Some(page) = history.pop() {
            self.page.replace(self.playback_page(page));
            self.filters.borrow_mut().clear();
            self.back.set_sensitive(!history.is_empty());
            drop(history);
            self.render();
        }
    }
    fn update_surface(&self, page: &Page) {
        let art = self.playback_artwork();
        let portrait = if *page == Page::Showcase {
            self.current()
                .and_then(|t| self.artist_portrait(&t.artist))
                .or_else(|| art.clone())
        } else {
            None
        };
        let next = match portrait {
            Some(image) => Surface::Portrait {
                image,
                art: art.clone(),
            },
            None => Surface::Artwork(art.clone()),
        };
        if self.background.borrow().as_deref() == Some(&next) {
            return;
        }
        let next = Rc::new(next);
        let wanted = Rc::downgrade(&next);
        self.background.replace(Some(next));
        let theme = self.theme.clone();
        let window = self.win.downgrade();
        let stack = self.surface.downgrade();
        let apply = move |sample: Option<visual::ArtSample>| {
            let (Some(surface), Some(stack), Some(window)) =
                (wanted.upgrade(), stack.upgrade(), window.upgrade())
            else {
                return;
            };
            let color = sample.as_ref().and_then(|s| s.color);
            theme.set_color(&window, color);
            let widget: gtk::Widget = match surface.as_ref() {
                Surface::Portrait { image, .. } => showcase::background(
                    image,
                    color,
                    window.width().max(window.height()) * window.scale_factor(),
                )
                .upcast(),
                Surface::Artwork(_) => visual::backdrop(sample).upcast(),
            };
            stack.add_child(&widget);
            stack.set_visible_child(&widget);
            visual::discard_old_backdrops(&stack);
        };
        if let Some(path) = art {
            glib::spawn_future_local(async move {
                let sample = gio::spawn_blocking(move || visual::sample(&path))
                    .await
                    .ok()
                    .flatten();
                apply(sample);
            });
        } else {
            apply(None);
        }
    }
    fn choose(self: &Rc<Self>, folder: bool) {
        let dialog = gtk::FileDialog::builder()
            .title(if folder {
                "Add a media folder"
            } else {
                "Open media file"
            })
            .build();
        let u = self.clone();
        let cb = move |r: Result<gio::File, glib::Error>| {
            if let Ok(f) = r
                && let Some(p) = f.path()
            {
                u.import(vec![p], !folder);
            }
        };
        if folder {
            dialog.select_folder(Some(&self.win), None::<&gio::Cancellable>, cb)
        } else {
            dialog.open(Some(&self.win), None::<&gio::Cancellable>, cb)
        }
    }
    fn queue_posters(&self, assets: &[PathBuf]) {
        let paths = assets
            .iter()
            .filter(|path| !media::is_photo_path(path))
            .cloned()
            .collect();
        let _ = self.poster_jobs.send(paths);
    }
    fn import(&self, paths: Vec<PathBuf>, play: bool) {
        self.notice("Reading media…");
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let assets = media::assets(&paths);
            let r = scan_paths(&paths, &data_dir().join("artwork"));
            if tx
                .send(Event::Scan(r.tracks, assets.clone(), r.warnings, play))
                .is_ok()
            {
                video_info::read_tags(tx, &assets);
            }
        });
    }
    fn event(self: &Rc<Self>, e: Event) {
        match e {
            Event::SaveError(message) => self.notice(&message),
            Event::PosterChanged(path) => {
                let gallery = self.videos.borrow().clone();
                if let Some(gallery) = gallery {
                    gallery.refresh_poster(&path);
                }
                if self.media.borrow().as_ref().is_some_and(|m| m.path == path) {
                    self.update_cover();
                }
            }
            Event::VideoTags(tags) => {
                let changed: Vec<_> = tags.iter().map(|(p, _)| p.clone()).collect();
                let mut db = self.db.borrow_mut();
                for (path, tag) in tags {
                    db.merge_video_tags(path, tag);
                }
                drop(db);
                self.persist();
                self.refresh_video_metadata(&changed);
            }
            Event::Scan(mut tracks, assets, warnings, play) => {
                self.queue_posters(&assets);
                let mut db = self.db.borrow_mut();
                let mut by: HashMap<_, _> =
                    db.tracks.drain(..).map(|t| (t.path.clone(), t)).collect();
                let mut newly_added = Vec::new();
                for t in &tracks {
                    if !by.contains_key(&t.path) {
                        newly_added.push(t.path.clone());
                    }
                    by.insert(t.path.clone(), t.clone());
                }
                newly_added.append(&mut db.added);
                db.added = newly_added;
                db.tracks = by.into_values().collect();
                let compare = |a: &Track, b: &Track| {
                    (
                        &a.album_artist,
                        &a.album,
                        a.disc_number,
                        a.track_number,
                        &a.title,
                        &a.path,
                    )
                        .cmp(&(
                            &b.album_artist,
                            &b.album,
                            b.disc_number,
                            b.track_number,
                            &b.title,
                            &b.path,
                        ))
                };
                db.tracks.sort_by(compare);
                tracks.sort_by(compare);
                for p in &assets {
                    if !db.assets.contains(p) {
                        db.assets.push(p.clone());
                    }
                }
                drop(db);
                self.update_cover();
                self.persist();
                self.notice(if warnings.is_empty() {
                    ""
                } else {
                    "Some media files or artwork could not be read completely."
                });
                if play && !tracks.is_empty() {
                    self.start(tracks, 0, None);
                    self.go(Page::Now)
                } else if play && assets.len() == 1 {
                    self.show_asset(&assets[0]);
                } else if !tracks.is_empty() {
                    self.go(Page::Collection("artists".into()))
                } else if let Some(p) = assets.first() {
                    self.go(Page::Collection(
                        if media::is_photo_path(p) {
                            "photos"
                        } else {
                            "videos"
                        }
                        .into(),
                    ));
                } else {
                    self.notice("No supported media found.");
                }
            }
            Event::Details(path, d) => {
                self.busy_enrich.set(false);
                self.details.borrow_mut().insert(path.clone(), d);
                if self
                    .current
                    .borrow()
                    .as_ref()
                    .is_some_and(|t| t.path == path)
                {
                    self.update_cover();
                    if matches!(
                        *self.page.borrow(),
                        Page::Now | Page::Artist(_) | Page::Lyrics | Page::Showcase
                    ) {
                        self.render();
                    }
                }
                self.enrich();
            }
            Event::Feed(feed, result) => match result {
                Ok(items) => {
                    self.feeds.borrow_mut().insert(feed, items);
                    self.notice("");
                    if matches!(*self.page.borrow(),Page::Collection(ref s) if s=="podcasts") {
                        self.render();
                    }
                }
                Err(e) => self.notice(&e),
            },
            Event::Podcast(title, result) => match result {
                Ok(path) => {
                    self.db.borrow_mut().episodes.insert(title, path);
                    self.persist();
                    self.notice("Episode downloaded.");
                    self.render();
                }
                Err(e) => self.notice(&e),
            },
            Event::Progress(path, secs) => {
                self.db
                    .borrow_mut()
                    .phone_progress
                    .insert(path.clone(), secs);
                self.persist();
                self.notice(&format!(
                    "Phone progress received for {} at {}",
                    path.file_stem().unwrap_or_default().to_string_lossy(),
                    time(secs)
                ));
                if *self.page.borrow() == Page::Devices {
                    self.render();
                }
            }
        }
    }
    fn current(&self) -> Option<Track> {
        self.embedded_track()
            .or_else(|| self.current.borrow().clone())
    }
    fn track(&self, p: &Path) -> Option<Track> {
        self.db
            .borrow()
            .tracks
            .iter()
            .find(|t| t.path == p)
            .cloned()
    }
    fn cached(&self, t: &Track) -> enrichment::Details {
        self.details
            .borrow()
            .get(&t.path)
            .cloned()
            .unwrap_or_else(|| enrichment::cached(t, &data_dir().join("online")))
    }
    fn cover(&self, t: &Track) -> Option<PathBuf> {
        self.db
            .borrow()
            .covers
            .get(&album_key(t))
            .cloned()
            .filter(|p| p.is_file())
            .or_else(|| t.cover.clone())
            .or_else(|| {
                self.details
                    .borrow()
                    .get(&t.path)
                    .and_then(|d| d.art.clone())
            })
            .or_else(|| enrichment::cached_cover(t, &data_dir().join("online")))
    }
    fn enrich(&self) {
        let Some(t) = self.current() else { return };
        if !self.db.borrow().enrichment
            || self.busy_enrich.get()
            || !self.requested.borrow_mut().insert(t.path.clone())
        {
            return;
        }
        let need_art = self.cover(&t).is_none();
        let need_lyrics = t.lyrics.is_none();
        self.busy_enrich.set(true);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let d = enrichment::lookup(&t, &data_dir().join("online"), need_art, need_lyrics);
            let _ = tx.send(Event::Details(t.path, d));
        });
    }
    fn update_cover(&self) {
        self.refresh_embedded_artwork();
        self.update_surface(&self.page.borrow());
        self.update_thumbnail();
    }
    fn toggle(self: &Rc<Self>) {
        if self.media.borrow().is_some() {
            self.command(self.set_playback_paused(!self.playback_state().paused));
            return;
        }
        if self.player.snapshot().finished && !self.queue.borrow().is_empty() {
            let queue = self.queue.borrow().clone();
            self.start(queue, 0, None);
            return;
        }
        if self.current().is_some() && !self.player.snapshot().finished {
            self.command(self.set_playback_paused(!self.player.snapshot().paused));
            return;
        }
        let db = self.db.borrow();
        let list: Vec<_> = db
            .recent_queue
            .iter()
            .filter_map(|p| db.tracks.iter().find(|t| &t.path == p).cloned())
            .collect();
        let list = if list.is_empty() {
            db.tracks.clone()
        } else {
            list
        };
        let n = db
            .current
            .as_ref()
            .and_then(|p| list.iter().position(|t| &t.path == p))
            .unwrap_or(0);
        let resume = list.get(n).and_then(|t| db.progress.get(&t.path)).copied();
        drop(db);
        if list.is_empty() {
            self.choose(true)
        } else {
            self.start(list, n, resume);
        }
    }
    fn start(self: &Rc<Self>, mut list: Vec<Track>, mut n: usize, resume: Option<f64>) {
        if n >= list.len() {
            return;
        }
        self.save_position();
        self.close_embedded();
        if *self.page.borrow() == Page::Video {
            self.go(Page::Now);
        }
        *self.sequence.borrow_mut() = list.clone();
        if self.shuffle.get() {
            let selected = list.remove(n);
            list.shuffle(&mut rand::rng());
            list.insert(0, selected);
            n = 0;
        }
        match self.player.set_queue(&list, n) {
            Ok(()) => {
                self.pending_seek.set(resume);
                self.db.borrow_mut().recent_queue = list.iter().map(|x| x.path.clone()).collect();
                *self.queue.borrow_mut() = list;
                self.notice("");
                self.persist_playback();
            }
            Err(e) => self.notice(&e),
        }
    }
    fn reshuffle(self: &Rc<Self>) {
        let q = self.sequence.borrow().clone();
        let paused = self.player.snapshot().paused;
        if let Some(t) = self.current()
            && let Some(i) = q.iter().position(|x| x.path == t.path)
        {
            self.start(q, i, Some(self.pos.get()));
            if paused {
                self.command(self.player.set_paused(true));
            }
        }
    }
    fn wake(&self) {
        self.last_input.set(Instant::now());
        self.bar.set_visible(true);
        if !self.mini.get() {
            self.nav.set_visible(true);
        }
    }
    fn fullscreen(&self) {
        if self.mini.get() {
            self.toggle_mini();
            if let Some(mut saved) = self.restoring_window.get() {
                saved.fullscreen = true;
                self.restoring_window.set(Some(saved));
            }
            self.wake();
            return;
        }
        if self.win.is_fullscreen() {
            self.win.unfullscreen();
            self.wake()
        } else {
            self.win.fullscreen();
            self.wake()
        }
    }
    fn toggle_mini(&self) {
        let mini = !self.mini.get();
        self.window_transition_started.set(Instant::now());
        if mini {
            let saved = self.restoring_window.take().unwrap_or(FullPlayerWindow {
                // GTK preserves this normal size while maximized/fullscreen.
                size: self.win.default_size(),
                maximized: self.win.is_maximized(),
                fullscreen: self.win.is_fullscreen(),
            });
            self.full_player_window.set(Some(saved));
        }
        self.mini.set(mini);
        if mini {
            self.win.add_css_class("mini");
            self.mini_action.set_label("Return to full player");
        } else {
            self.win.remove_css_class("mini");
            self.mini_action.set_label("Mini player");
        }
        self.nav.set_visible(!mini);
        self.motion.set_visible(!mini);
        self.update_thumbnail();
        self.wake();
        if mini {
            self.fitting_mini.set(true);
            self.fit_mini_window();
        } else if let Some(saved) = self.full_player_window.take() {
            self.fitting_mini.set(false);
            self.restoring_window.set(Some(saved));
            self.win.set_default_size(saved.size.0, saved.size.1);
        }
    }
    fn fit_mini_window(&self) {
        if self.mini.get() && self.fitting_mini.get() {
            if self.window_transition_expired() {
                self.fitting_mini.set(false);
                return;
            }
            if self.win.is_fullscreen() {
                self.win.unfullscreen();
            }
            if self.win.is_maximized() {
                self.win.unmaximize();
            }
            // A state notification can precede the compositor's old-size
            // configure. Keep fitting until the minimum is actually allocated.
            let (width, height) = self.window_minimum();
            if !self.win.is_fullscreen()
                && !self.win.is_maximized()
                && self.win.width() == width
                && self.win.height() == height
            {
                self.fitting_mini.set(false);
            } else {
                self.win.set_default_size(1, 1);
            }
        }
    }
    fn window_minimum(&self) -> (i32, i32) {
        let width = self.win.measure(gtk::Orientation::Horizontal, -1).0;
        let height = self.win.measure(gtk::Orientation::Vertical, width).0;
        (width, height)
    }
    fn window_transition_expired(&self) -> bool {
        // A compositor can constrain geometry (for example, on a small monitor
        // or when tiled). Release sizing control after this brief transition.
        self.window_transition_started.get().elapsed() >= Duration::from_millis(1200)
    }
    fn settle_window_mode(&self) {
        self.fit_mini_window();
        if let Some(saved) = self.restoring_window.get() {
            let minimum = self.window_minimum();
            let target = (saved.size.0.max(minimum.0), saved.size.1.max(minimum.1));
            if self.win.is_fullscreen() {
                self.win.unfullscreen();
            }
            if self.win.is_maximized() {
                self.win.unmaximize();
            }
            if self.window_transition_expired()
                || (!self.win.is_fullscreen()
                    && !self.win.is_maximized()
                    && (self.win.width(), self.win.height()) == target)
            {
                // Commit normal bounds before maximizing/fullscreening so the
                // compositor restores these bounds rather than mini geometry.
                self.restoring_window.take();
                if saved.maximized {
                    self.win.maximize();
                }
                if saved.fullscreen {
                    self.win.fullscreen();
                }
            } else {
                self.win.set_default_size(saved.size.0, saved.size.1);
            }
        }
    }
    fn window_mode_changed(&self) {
        if self.fitting_mini.get() {
            self.fit_mini_window();
        } else if self.mini.get() && (self.win.is_maximized() || self.win.is_fullscreen()) {
            // A later user maximize/fullscreen request reveals the full UI.
            let maximized = self.win.is_maximized();
            let fullscreen = self.win.is_fullscreen();
            self.toggle_mini();
            if let Some(mut saved) = self.restoring_window.get() {
                saved.maximized = maximized;
                saved.fullscreen = fullscreen;
                self.restoring_window.set(Some(saved));
            }
        }
    }
    fn save_position(&self) {
        if let Some(path) = self.media.borrow().as_ref().map(|m| m.path.clone()) {
            let active = self.playback_state();
            self.db
                .borrow_mut()
                .progress
                .insert(path, if active.finished { 0. } else { active.position });
            return;
        }
        if let Some(t) = self.current() {
            let pos = if self.playback_state().finished
                || (t.duration > 0. && self.pos.get() >= t.duration - 0.2)
            {
                0.
            } else {
                self.pos.get()
            };
            self.db.borrow_mut().progress.insert(t.path, pos);
        }
    }
    fn tick(self: &Rc<Self>) {
        self.settle_window_mode();
        let compact = self.win.width() > 0 && self.win.width() < 780;
        if self.compact.replace(compact) != compact {
            if compact {
                self.win.add_css_class("compact");
            } else {
                self.win.remove_css_class("compact");
            }
            if let Some(browser) = self.browser.borrow().upgrade() {
                browser.set_compact(compact);
            }
            if matches!(*self.page.borrow(), Page::Now | Page::Wall | Page::Showcase) {
                self.render();
            }
        }
        let s = self.playback_state();
        if s.finished && self.advance_finished_video() {
            return;
        }
        if let Some(e) = s.error.as_ref()
            && self.last_error.borrow().as_ref() != Some(e)
        {
            self.notice(e);
            *self.last_error.borrow_mut() = Some(e.clone());
        }
        if self.media.borrow().is_none()
            && let Some(id) = s.track_id.as_ref()
            && self.current.borrow().as_ref().is_none_or(|t| &t.id != id)
            && let Some(t) = self.queue.borrow().iter().find(|t| &t.id == id).cloned()
        {
            self.save_position();
            *self.current.borrow_mut() = Some(t.clone());
            let mut db = self.db.borrow_mut();
            state::promote(&mut db.history, &t.path);
            db.current = Some(t.path.clone());
            drop(db);
            self.persist_playback();
            let cached = self.cached(&t);
            self.details.borrow_mut().insert(t.path.clone(), cached);
            self.mini_title
                .set_text(&format!("{} · {}", t.artist, t.title));
            self.mini_title
                .set_tooltip_text(Some(&format!("{} · {}", t.artist, t.album)));
            if let Some(browser) = self.browser.borrow().upgrade() {
                browser.highlight(Some(&t));
            }
            self.update_cover();
            self.enrich();
            if !matches!(*self.page.borrow(), Page::Collection(_)) {
                self.render();
            }
        }
        if let Some(media) = self.media.borrow().as_ref() {
            if self.mini_title.text() != media.title {
                self.mini_title.set_text(&media.title);
            }
            if self.mini_title.tooltip_text().as_deref() != Some(media.title.as_str()) {
                self.mini_title.set_tooltip_text(Some(&media.title));
            }
        }
        self.pos.set(s.position);
        self.duration.set(s.duration);
        let timed = s.track_id.is_some() && s.duration > 0.;
        self.seek.set_sensitive(timed);
        self.elapsed.set_visible(timed);
        self.remaining.set_visible(timed);
        if self.seek.adjustment().upper() != s.duration.max(1.) {
            self.seek.set_range(0., s.duration.max(1.));
        }
        let icon = if s.paused || s.finished || s.track_id.is_none() {
            "media-playback-start-symbolic"
        } else {
            "media-playback-pause-symbolic"
        };
        if self.play.icon_name().as_deref() != Some(icon) {
            self.play.set_icon_name(icon);
        }
        let now = glib::monotonic_time();
        let sample_time = now
            - s.position_updated
                .elapsed()
                .as_micros()
                .min(i64::MAX as u128) as i64;
        self.playhead
            .borrow_mut()
            .sample(&s, self.playback_rate(), sample_time, now);
        self.draw_playhead(now);
        self.sync_playhead_tick(now);
        if self.media.borrow().is_none()
            && s.loaded
            && s.duration > 0.
            && let Some(n) = self.pending_seek.take()
        {
            self.command(self.seek_playback(n));
        }
        if self.media.borrow().is_none() && s.finished && self.repeat.get() {
            let q = self.queue.borrow().clone();
            self.start(q, 0, None);
        }
        if self.win.is_fullscreen()
            && self.last_input.get().elapsed() > Duration::from_secs(4)
            && matches!(
                *self.page.borrow(),
                Page::Now | Page::Wall | Page::Showcase | Page::Video
            )
        {
            self.bar.set_visible(false);
            self.nav.set_visible(false);
        } else if !self.win.is_fullscreen() && !self.mini.get() {
            self.bar.set_visible(true);
            self.nav.set_visible(true);
        }
    }
    // Only visible transport/lyric changes belong on the display frame clock.
    // Backend sampling, persistence, library work and rendering stay in tick().
    fn draw_playhead(&self, frame_time: i64) {
        let position = self.playhead.borrow().position(frame_time);
        if self.seek.value() != position {
            self.seek.set_value(position);
        }
        let elapsed = time(position);
        if self.elapsed.text() != elapsed {
            self.elapsed.set_text(&elapsed);
        }
        let remaining = format!("−{}", time(self.duration.get() - position));
        if self.remaining.text() != remaining {
            self.remaining.set_text(&remaining);
        }
        if *self.page.borrow() == Page::Lyrics {
            self.highlight_lyric(position);
        }
    }
    fn playhead_visible(&self) -> bool {
        self.win.is_mapped() && (self.bar.is_visible() || *self.page.borrow() == Page::Lyrics)
    }
    fn sync_playhead_tick(self: &Rc<Self>, now: i64) {
        let animate = self.playhead_visible() && self.playhead.borrow().animating(now);
        if !animate {
            if let Some(tick) = self.playhead_tick.take() {
                tick.remove();
            }
        } else if self.playhead_tick.borrow().is_none() {
            let weak = Rc::downgrade(self);
            let tick = self.win.add_tick_callback(move |_, clock| {
                let Some(ui) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let now = clock.frame_time();
                ui.draw_playhead(now);
                if ui.playhead_visible() && ui.playhead.borrow().animating(now) {
                    glib::ControlFlow::Continue
                } else {
                    ui.playhead_tick.take();
                    glib::ControlFlow::Break
                }
            });
            self.playhead_tick.replace(Some(tick));
        }
    }
    fn choose_cover(self: &Rc<Self>, t: Track) {
        let v = self.clone();
        gtk::FileDialog::builder()
            .title("Choose album artwork")
            .build()
            .open(Some(&self.win), None::<&gio::Cancellable>, move |r| {
                if let Ok(f) = r
                    && let Some(p) = f.path()
                {
                    if visual::palette(&p).is_none() {
                        v.notice("Choose a readable image.");
                        return;
                    }
                    match gnune::library::save_image(&p, &data_dir().join("artwork")) {
                        Ok(file) => {
                            v.db.borrow_mut().covers.insert(album_key(&t), file);
                        }
                        Err(e) => {
                            v.notice(&e.to_string());
                            return;
                        }
                    }
                    v.persist();
                    v.update_cover();
                    v.render();
                }
            });
    }
    fn prompt(
        self: &Rc<Self>,
        title: &str,
        submit: &str,
        cb: impl Fn(&Rc<Self>, String) + 'static,
    ) {
        let w = gtk::Window::builder()
            .title(title)
            .transient_for(&self.win)
            .modal(true)
            .default_width(410)
            .build();
        let b = col(14);
        b.add_css_class("dialog");
        let e = gtk::Entry::new();
        e.set_placeholder_text(Some(title));
        b.append(&e);
        let v = self.clone();
        let w2 = w.clone();
        let go = button(submit, move || {
            let text = e.text().trim().to_string();
            if !text.is_empty() {
                cb(&v, text);
                w2.close();
            }
        });
        b.append(&go);
        chrome::set_content(&w, &b);
        w.present();
    }
}
