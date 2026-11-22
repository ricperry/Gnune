//! Content-only window chrome shared by every application-owned GTK window.
use gtk::{gdk, glib, prelude::*};
use std::{cell::Cell, cell::RefCell, rc::Rc};

const INSET: i32 = 8;
const EDGE: f64 = 6.;

struct Controls {
    revealer: glib::WeakRef<gtk::Revealer>,
    buttons: Vec<glib::WeakRef<gtk::Button>>,
    proximity: Cell<f64>,
    focused: Cell<bool>,
    previous_focus: RefCell<Option<glib::WeakRef<gtk::Widget>>>,
}

impl Controls {
    fn update(&self) {
        let opacity = if self.focused.get() {
            1.
        } else {
            self.proximity.get()
        };
        let show = opacity > 0.;
        if let Some(revealer) = self.revealer.upgrade() {
            if show && let Some(group) = revealer.child() {
                group.set_opacity(opacity);
            }
            // Stop targeting immediately on fade-out, even while still drawn.
            revealer.set_can_target(show);
            for button in self.buttons.iter().filter_map(glib::WeakRef::upgrade) {
                button.set_focusable(show);
            }
            revealer.set_reveal_child(show);
        }
    }
}

pub fn set_content(window: &impl IsA<gtk::Window>, content: &impl IsA<gtk::Widget>) {
    let window = window.as_ref();
    window.set_decorated(false);
    let overlay = gtk::Overlay::new();
    // GTK supplies native move, double-click maximize, and window-menu behavior
    // on unused content, while interactive descendants handle their own input.
    let handle = gtk::WindowHandle::new();
    handle.set_child(Some(content));
    overlay.set_child(Some(&handle));

    let group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    group.add_css_class("window-controls");
    let mut buttons = Vec::new();
    for (icon, label) in [
        ("window-minimize-symbolic", "Minimize"),
        ("window-maximize-symbolic", "Maximize"),
        ("window-close-symbolic", "Close"),
    ] {
        let button = gtk::Button::from_icon_name(icon);
        button.set_tooltip_text(Some(label));
        button.update_property(&[gtk::accessible::Property::Label(label)]);
        button.set_focusable(false);
        button.set_focus_on_click(false);
        group.append(&button);
        buttons.push(button);
    }
    buttons[2].add_css_class("close");
    let weak = window.downgrade();
    buttons[0].connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.minimize();
        }
    });
    let weak = window.downgrade();
    buttons[1].connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            if window.is_fullscreen() {
                window.unfullscreen();
            } else if window.is_maximized() {
                window.unmaximize();
            } else {
                window.maximize();
            }
        }
    });
    let weak = window.downgrade();
    buttons[2].connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
    });
    let maximize = buttons[1].downgrade();
    let update_maximize = move |window: &gtk::Window| {
        if let Some(button) = maximize.upgrade() {
            let restore = window.is_maximized() || window.is_fullscreen();
            let label = if restore { "Restore" } else { "Maximize" };
            button.set_icon_name(if restore {
                "window-restore-symbolic"
            } else {
                "window-maximize-symbolic"
            });
            button.set_tooltip_text(Some(label));
            button.update_property(&[gtk::accessible::Property::Label(label)]);
            button.set_sensitive(window.is_resizable() || restore);
        }
    };
    update_maximize(window);
    window.connect_maximized_notify(update_maximize.clone());
    window.connect_fullscreened_notify(update_maximize.clone());
    window.connect_resizable_notify(update_maximize);

    let revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::Crossfade)
        .transition_duration(160)
        .halign(gtk::Align::End)
        .valign(gtk::Align::Start)
        .margin_top(INSET)
        .margin_end(INSET)
        .can_target(false)
        .child(&group)
        .build();
    // GtkRevealer observes gtk-enable-animations, including reduced motion.
    overlay.add_overlay(&revealer);
    overlay.set_measure_overlay(&revealer, false);
    let controls = Rc::new(Controls {
        revealer: revealer.downgrade(),
        buttons: buttons.iter().map(ObjectExt::downgrade).collect(),
        proximity: Cell::new(0.),
        focused: Cell::new(false),
        previous_focus: RefCell::new(None),
    });
    let focus = gtk::EventControllerFocus::new();
    let state = controls.clone();
    focus.connect_contains_focus_notify(move |focus| {
        state.focused.set(focus.contains_focus());
        state.update();
    });
    group.add_controller(focus);

    let motion = gtk::EventControllerMotion::new();
    motion.set_propagation_phase(gtk::PropagationPhase::Capture);
    let state = controls.clone();
    let weak = window.downgrade();
    let update_pointer = move |_: &gtk::EventControllerMotion, x, y| {
        if let Some(window) = weak.upgrade() {
            state
                .proximity
                .set(control_opacity(window.width() as f64, x, y));
            state.update();
            let edge = resize_edge(&window, x, y);
            window.set_cursor_from_name(edge.map(|(_, cursor)| cursor));
        }
    };
    motion.connect_enter(update_pointer.clone());
    motion.connect_motion(update_pointer);
    let state = controls.clone();
    let weak = window.downgrade();
    motion.connect_leave(move |_| {
        state.proximity.set(0.);
        state.update();
        if let Some(window) = weak.upgrade() {
            window.set_cursor_from_name(None);
        }
    });
    window.add_controller(motion);

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let state = controls;
    let weak = window.downgrade();
    keys.connect_key_pressed(move |_, key, _, mods| {
        let Some(window) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        if key == gdk::Key::F10 && mods.is_empty() {
            if !state.focused.get() {
                *state.previous_focus.borrow_mut() =
                    gtk::prelude::GtkWindowExt::focus(&window).map(|w| w.downgrade());
                state.focused.set(true);
                state.update();
                if let Some(button) = state.buttons[0].upgrade() {
                    button.grab_focus();
                }
            }
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::Escape && state.focused.get() {
            let previous = state
                .previous_focus
                .borrow_mut()
                .take()
                .and_then(|w| w.upgrade());
            gtk::prelude::GtkWindowExt::set_focus(&window, previous.as_ref());
            state.focused.set(false);
            state.update();
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::Escape && window.is_fullscreen() {
            window.unfullscreen();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys);

    let resize = gtk::GestureClick::new();
    resize.set_button(1);
    resize.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = window.downgrade();
    resize.connect_pressed(move |gesture, _, x, y| {
        let Some(window) = weak.upgrade() else { return };
        let Some((edge, _)) = resize_edge(&window, x, y) else {
            return;
        };
        if let (Some(surface), Some(device), Some(event)) = (
            window.surface().and_downcast::<gdk::Toplevel>(),
            gesture.current_event_device(),
            gesture.current_event(),
        ) && let Some((surface_x, surface_y)) = event.position()
        {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            surface.begin_resize(edge, Some(&device), 1, surface_x, surface_y, event.time());
        }
    });
    window.add_controller(resize);
    window.set_child(Some(&overlay));
}

fn control_opacity(width: f64, x: f64, y: f64) -> f64 {
    // Distance from the top-right control group. Inside the group every icon
    // is fully legible. In the surrounding 44px the three fade together.
    let dx = (width - 104. - x).max(0.);
    let dy = (y - 40.).max(0.);
    (1. - dx.hypot(dy) / 44.).clamp(0., 1.)
}

fn resize_edge(window: &gtk::Window, x: f64, y: f64) -> Option<(gdk::SurfaceEdge, &'static str)> {
    if !window.is_resizable() || window.is_fullscreen() || window.is_maximized() {
        return None;
    }
    edge_at(window.width() as f64, window.height() as f64, x, y)
}

fn edge_at(width: f64, height: f64, x: f64, y: f64) -> Option<(gdk::SurfaceEdge, &'static str)> {
    use gdk::SurfaceEdge::*;
    let left = x < EDGE;
    let right = x >= width - EDGE;
    let top = y < EDGE;
    let bottom = y >= height - EDGE;
    match (left, right, top, bottom) {
        (true, _, true, _) => Some((NorthWest, "nw-resize")),
        (_, true, true, _) => Some((NorthEast, "ne-resize")),
        (true, _, _, true) => Some((SouthWest, "sw-resize")),
        (_, true, _, true) => Some((SouthEast, "se-resize")),
        (true, _, _, _) => Some((West, "w-resize")),
        (_, true, _, _) => Some((East, "e-resize")),
        (_, _, true, _) => Some((North, "n-resize")),
        (_, _, _, true) => Some((South, "s-resize")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_resize_edge_and_corner_is_available_without_covering_content() {
        use gdk::SurfaceEdge::*;
        for (x, y, expected) in [
            (0., 0., NorthWest),
            (400., 0., North),
            (799., 0., NorthEast),
            (0., 300., West),
            (799., 300., East),
            (0., 599., SouthWest),
            (400., 599., South),
            (799., 599., SouthEast),
        ] {
            assert_eq!(edge_at(800., 600., x, y).map(|e| e.0), Some(expected));
        }
        assert!(edge_at(800., 600., 8., 8.).is_none());
        assert!(edge_at(800., 600., 400., 300.).is_none());
    }

    #[test]
    fn proximity_tracks_window_size_including_mini_player() {
        assert_eq!(control_opacity(1100., 1090., 40.), 1.);
        assert_eq!(control_opacity(1100., 700., 40.), 0.);
        assert_eq!(control_opacity(1100., 1090., 120.), 0.);
        assert_eq!(control_opacity(680., 670., 40.), 1.);
        assert_eq!(control_opacity(680., 300., 40.), 0.);
        assert_eq!(control_opacity(1100., 974., 30.), 0.5);
    }
}
