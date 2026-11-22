//! The artist portrait, diagonal typography and queue share a single listening surface.
use super::*;

pub fn background(path: &Path, color: Option<(u8, u8, u8)>, size: i32) -> gtk::Overlay {
    let surface = gtk::Overlay::new();
    let photo = gtk::Picture::new();
    image_loader::load(&photo, path, size);
    photo.set_can_shrink(true);
    photo.set_content_fit(gtk::ContentFit::Cover);
    surface.set_child(Some(&photo));
    let (r, g, b) = color.unwrap_or((160, 140, 105));
    let tint = gtk::gdk::RGBA::new(r as f32 / 255., g as f32 / 255., b as f32 / 255., 0.32);
    let veil = gtk::Picture::new();
    let snapshot = gtk::Snapshot::new();
    let bounds = gtk::graphene::Rect::new(0., 0., 1., 1.);
    snapshot.append_color(&tint, &bounds);
    let stops = [(0., 0.64), (0.38, 0.20), (1., 0.74)].map(|(at, alpha)| {
        gtk::gsk::ColorStop::new(at, gtk::gdk::RGBA::new(0.045, 0.035, 0.05, alpha))
    });
    snapshot.append_linear_gradient(
        &bounds,
        &gtk::graphene::Point::zero(),
        &gtk::graphene::Point::new(0.2, 1.),
        &stops,
    );
    veil.set_paintable(snapshot.to_paintable(None).as_ref());
    veil.set_content_fit(gtk::ContentFit::Fill);
    veil.set_can_shrink(true);
    surface.add_overlay(&veil);
    surface.set_vexpand(true);
    surface
}

mod lettering {
    use gtk::{glib, prelude::*, subclass::prelude::*};
    use std::cell::OnceCell;

    #[derive(Default)]
    pub struct Lettering(pub OnceCell<[String; 3]>);

    #[glib::object_subclass]
    impl ObjectSubclass for Lettering {
        const NAME: &'static str = "GnuneShowcaseLettering";
        type Type = super::Lettering;
        type ParentType = gtk::Widget;
    }
    impl ObjectImpl for Lettering {}
    impl WidgetImpl for Lettering {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let Some(words) = self.0.get() else { return };
            let w = widget.width() as f32;
            let h = widget.height() as f32;
            let size = (w * 0.12).min(h * 0.23);
            snapshot.translate(&gtk::graphene::Point::new(w * 0.11, h * 0.56));
            snapshot.rotate(-34.);
            for (text, scale, offset, alpha) in [
                (&words[0], 0.32, -0.38, 0.58),
                (&words[1], 1.0, 0.0, 0.29),
                (&words[2], 0.30, 1.17, 0.65),
            ] {
                let layout = widget.create_pango_layout(Some(text));
                let mut font = gtk::pango::FontDescription::from_string("Segoe UI Bold");
                font.set_absolute_size(f64::from(size * scale) * f64::from(gtk::pango::SCALE));
                layout.set_font_description(Some(&font));
                snapshot.save();
                snapshot.translate(&gtk::graphene::Point::new(0., size * offset));
                snapshot.append_layout(&layout, &gtk::gdk::RGBA::new(1., 1., 1., alpha));
                snapshot.restore();
            }
        }
    }
}
glib::wrapper! {
    pub struct Lettering(ObjectSubclass<lettering::Lettering>)
        @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

pub fn compose(ui: &Rc<Ui>, overlay: &gtk::Overlay, t: &Track) {
    use gtk::subclass::prelude::ObjectSubclassIsExt;
    let type_layer: Lettering = glib::Object::new();
    type_layer.set_can_target(false);
    type_layer.set_accessible_role(gtk::AccessibleRole::Presentation);
    let _ = type_layer.imp().0.set([
        t.artist.to_uppercase(),
        t.year
            .map(|y| y.to_string())
            .unwrap_or_else(|| time(t.duration)),
        t.title.to_uppercase(),
    ]);
    overlay.add_overlay(&type_layer);
    if ui.compact.get() {
        return;
    }
    let queue = ui.queue.borrow();
    if queue.is_empty() {
        return;
    }
    let panel = col(4);
    panel.add_css_class("showcase-queue");
    panel.set_width_request(192);
    panel.set_margin_end(20);
    panel.set_margin_bottom(22);
    panel.set_margin_top(26);
    panel.set_halign(gtk::Align::End);
    panel.set_valign(gtk::Align::End);
    panel.append(&label("now playing", "queue-name"));
    let active = ui.player.snapshot().queue_index.unwrap_or(0);
    for (index, item) in queue
        .iter()
        .enumerate()
        .skip(active.saturating_sub(1))
        .take(6)
    {
        let names = col(0);
        let track = label(&item.title, "track");
        track.set_width_chars(17);
        track.set_max_width_chars(20);
        names.append(&track);
        let artist = label(&item.artist, "small");
        artist.set_max_width_chars(21);
        names.append(&artist);
        let b = app_button(ui, "", move |u| {
            u.save_position();
            u.close_embedded();
            if *u.page.borrow() == Page::Video {
                u.go(Page::Now);
            }
            u.command(u.player.play_at(index));
        });
        b.set_child(Some(&names));
        if index == active {
            b.add_css_class("playing");
        }
        panel.append(&b);
    }
    panel.append(&app_button(ui, "View queue", |u| u.go(Page::Queue)));
    overlay.add_overlay(&panel);
}
