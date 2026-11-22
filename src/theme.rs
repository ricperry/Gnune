//! GTK animates the playback palette across the application's windows.
use gtk::{glib, prelude::*};
use std::{cell::Cell, fmt, rc::Rc};

pub const FADE_MS: u32 = 1500;
const NEUTRAL: Rgb = Rgb(118, 93, 112);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rgb(u8, u8, u8);

impl Rgb {
    fn mix(self, other: Self, amount: f64) -> Self {
        let channel = |a, b| (f64::from(a) + (f64::from(b) - f64::from(a)) * amount).round() as u8;
        Self(
            channel(self.0, other.0),
            channel(self.1, other.1),
            channel(self.2, other.2),
        )
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

struct Palette {
    canvas: Rgb,
    panel: Rgb,
    ink: Rgb,
    muted: Rgb,
    line: Rgb,
    accent: Rgb,
}

impl Palette {
    fn from_art(seed: Rgb) -> Self {
        let canvas = Rgb(14, 15, 19).mix(seed, 0.17);
        let ink = Rgb(251, 249, 250).mix(seed, 0.035);
        Self {
            canvas,
            panel: canvas.mix(ink, 0.075),
            ink,
            muted: ink.mix(canvas, 0.25),
            line: ink.mix(canvas, 0.69),
            accent: seed.mix(Rgb(255, 246, 248), 0.55),
        }
    }

    fn css(&self) -> String {
        let Self {
            canvas,
            panel,
            ink,
            muted,
            line,
            accent,
        } = self;
        format!(
            "@define-color gn_canvas {canvas}; @define-color gn_panel {panel}; \
                 @define-color gn_ink {ink}; @define-color gn_muted {muted}; \
                 @define-color gn_line {line}; @define-color gn_accent {accent};"
        )
    }
}

// Animate color only where it is explicitly assigned. Animating an inherited color a
// second time would restart the child transition on each parent frame. Keep palette
// nodes here; animate paint properties without transitioning widget layout.
const COLOR_NODES: &str = r#".clock,
.collection-album-artist,
.collection-album-title,
.collection-album.selected,
.collection-artist-list row:selected,
.collection-categories button,
.collection-categories button.selected,
.collection-count,
.collection-details,
.collection-empty,
.collection-modes button,
.collection-modes button.selected,
.collection-order,
.collection-reset,
.collection-song-list row:selected,
.current-line,
.eyebrow,
.hero-album,
.hero-artist,
.lyric,
.nav .pivot,
.nav .pivot.selected,
.primary,
.quickplay-add,
.quickplay-copy,
.quickplay-dj-label,
.quickplay-dj-seed,
.quickplay-section-title,
.quickplay-welcome .quickplay-section-title,
.selected,
.showcase-identity,
.showcase-queue,
.showcase-queue .playing .track,
.small,
.status,
.subtitle,
.transport .play,
.transport .round,
.video-info-link,
.video-poster,
.window-controls,
button.link,
checkbutton check:checked,
dropdown > button,
dropdown listview,
dropdown listview row:selected,
entry,
entry selection,
link,
passwordentry,
popover button:hover,
popover contents,
searchentry,
spinbutton,
textview,
textview selection,
textview text,
window,
window.metadata-dialog"#;
const SURFACE_NODES: &str = r#".collection-albums scrolledwindow,
.collection-artists scrolledwindow,
.collection-panes separator,
.item,
.nav,
.quickplay-tile,
.video-play,
.window-controls button,
button,
dropdown,
scale,
scale highlight,
scale slider,
scale trough"#;

fn transitions(milliseconds: u32) -> String {
    let surfaces = [
        "background-color",
        "background-image",
        "border-color",
        "outline-color",
    ]
    .map(|property| format!("{property} {milliseconds}ms ease-in-out"))
    .join(", ");
    format!(
        "{SURFACE_NODES} {{ transition: {surfaces}; }}\n\
             {COLOR_NODES} {{ transition: color {milliseconds}ms ease-in-out, {surfaces}; }}"
    )
}

pub struct Theme {
    display: gtk::gdk::Display,
    armed: Cell<bool>,
    colors: gtk::CssProvider,
    transition: gtk::CssProvider,
    target: Cell<Rgb>,
    revision: Cell<u64>,
}

impl Theme {
    pub fn new(display: &gtk::gdk::Display) -> Rc<Self> {
        let colors = gtk::CssProvider::new();
        colors.load_from_string(&Palette::from_art(NEUTRAL).css());
        gtk::style_context_add_provider_for_display(display, &colors, 610);
        let normal = gtk::CssProvider::new();
        normal.load_from_string(&transitions(100));
        gtk::style_context_add_provider_for_display(display, &normal, 611);
        let transition = gtk::CssProvider::new();
        transition.load_from_string(&transitions(FADE_MS));
        Rc::new(Self {
            display: display.clone(),
            armed: Cell::new(false),
            colors,
            transition,
            target: Cell::new(NEUTRAL),
            revision: Cell::new(0),
        })
    }

    fn disarm(&self, revision: u64) {
        if self.revision.get() == revision && self.armed.replace(false) {
            gtk::style_context_remove_provider_for_display(&self.display, &self.transition);
        }
    }

    pub fn set_color(
        self: &Rc<Self>,
        window: &gtk::ApplicationWindow,
        color: Option<(u8, u8, u8)>,
    ) {
        let target = color.map_or(NEUTRAL, |(r, g, b)| Rgb(r, g, b));
        if self.target.replace(target) == target {
            return;
        }
        let revision = self.revision.get().wrapping_add(1);
        self.revision.set(revision);
        if window.is_mapped() && window.settings().is_gtk_enable_animations() {
            // Only the palette update starts a long transition. After GTK has resolved one
            // frame, restore short durations for hover, selection, and focus changes. GTK
            // retains transitions whose destination is unchanged, including rapid retargets.
            if !self.armed.replace(true) {
                gtk::style_context_add_provider_for_display(&self.display, &self.transition, 612);
            }
            let theme = self.clone();
            let frames = Cell::new(0);
            window.add_tick_callback(move |_, _| {
                if theme.revision.get() != revision {
                    return glib::ControlFlow::Break;
                }
                frames.set(frames.get() + 1);
                if frames.get() < 2 {
                    return glib::ControlFlow::Continue;
                }
                theme.disarm(revision);
                glib::ControlFlow::Break
            });
            // A hidden/minimized window can stop producing frames between the two ticks.
            let theme = self.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(100), move || {
                theme.disarm(revision);
            });
        } else {
            self.disarm(revision);
        }
        // Parse the palette once. GTK interpolates its properties on the display's frame clock.
        self.colors
            .load_from_string(&Palette::from_art(target).css());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contrast(a: Rgb, b: Rgb) -> f64 {
        let luminance = |c: Rgb| {
            let linear = |v| {
                let t = f64::from(v) / 255.;
                if t <= 0.04045 {
                    t / 12.92
                } else {
                    ((t + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(c.0) + 0.7152 * linear(c.1) + 0.0722 * linear(c.2)
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn palette_keeps_dark_surfaces_and_legible_credits() {
        for r in (0..=255).step_by(51) {
            for g in (0..=255).step_by(51) {
                for b in (0..=255).step_by(51) {
                    let p = Palette::from_art(Rgb(r, g, b));
                    assert!(p.canvas.0.max(p.canvas.1).max(p.canvas.2) <= 60);
                    for fg in [p.ink, p.muted, p.accent] {
                        assert!(contrast(fg, p.canvas) >= 4.5);
                        assert!(contrast(fg, p.panel) >= 4.5);
                    }
                }
            }
        }
    }
}
