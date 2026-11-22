//! Frame-clock motion over GTK render nodes. Layout and image loading stay outside frames.
use gtk::{gdk, glib, graphene, gsk, prelude::*, subclass::prelude::*};
use std::cell::{Cell, RefCell};

const DURATION_US: f64 = 460_000.;

#[derive(Clone)]
pub struct Art {
    node: gsk::RenderNode,
    paintable: gdk::Paintable,
    bounds: graphene::Rect,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Stage {
        pub child: RefCell<Option<gtk::Widget>>,
        pub frame: RefCell<Option<gsk::RenderNode>>,
        pub old: RefCell<Option<gsk::RenderNode>>,
        pub source: RefCell<Option<Art>>,
        pub explicit_source: RefCell<Option<(std::path::PathBuf, Art)>>,
        pub target_key: RefCell<Option<std::path::PathBuf>>,
        pub target: RefCell<Option<glib::WeakRef<gtk::Widget>>>,
        pub start: Cell<i64>,
        pub progress: Cell<f64>,
        pub zoom: Cell<bool>,
        pub tick: RefCell<Option<gtk::TickCallbackId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Stage {
        const NAME: &'static str = "GnuneMotionStage";
        type Type = super::Stage;
        type ParentType = gtk::Widget;
    }
    impl ObjectImpl for Stage {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().set_layout_manager(Some(gtk::BinLayout::new()));
        }
        fn dispose(&self) {
            if let Some(tick) = self.tick.take() {
                tick.remove();
            }
            if let Some(child) = self.child.take() {
                child.unparent();
            }
        }
    }
    impl WidgetImpl for Stage {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let Some(child) = self.child.borrow().clone() else {
                return;
            };
            let frame = gtk::Snapshot::new();
            let p = ease(self.progress.get());
            if let Some(old) = self.old.borrow().as_ref() {
                frame.save();
                frame.push_clip(&graphene::Rect::new(
                    0.,
                    0.,
                    obj.width() as f32,
                    obj.height() as f32,
                ));
                frame.push_opacity(1. - p);
                frame.translate(&graphene::Point::new(-36. * p as f32, 0.));
                frame.append_node(old);
                frame.pop();
                frame.pop();
                frame.restore();
                frame.save();
                frame.push_clip(&graphene::Rect::new(
                    0.,
                    0.,
                    obj.width() as f32,
                    obj.height() as f32,
                ));
                frame.push_opacity(p);
                // Sliding collection text at its final size reuses the rendered glyphs.
                frame.translate(&graphene::Point::new(76. * (1. - p) as f32, 0.));
                if self.zoom.get() {
                    let scale = (0.955 + 0.045 * p) as f32;
                    frame.scale(scale, scale);
                }
                obj.snapshot_child(&child, &frame);
                frame.pop();
                frame.pop();
                frame.restore();
                if let Some(source) = self.source.borrow().as_ref()
                    && let Some(target) = self
                        .target
                        .borrow()
                        .as_ref()
                        .and_then(glib::WeakRef::upgrade)
                    && let Some(bounds) = target.compute_bounds(&*obj)
                    && bounds.width() > 0.
                    && bounds.height() > 0.
                {
                    let to = picture_bounds(&target, bounds);
                    let from = &source.bounds;
                    let x = lerp(from.x(), to.x(), p);
                    let y = lerp(from.y(), to.y(), p);
                    let w = lerp(from.width(), to.width(), p);
                    let h = lerp(from.height(), to.height(), p);
                    frame.save();
                    frame.translate(&graphene::Point::new(x, y));
                    frame.scale(w / from.width(), h / from.height());
                    frame.append_node(&source.node);
                    frame.restore();
                }
            } else {
                obj.snapshot_child(&child, &frame);
            }
            if let Some(node) = frame.to_node() {
                snapshot.append_node(&node);
                self.frame.replace(Some(node));
            }
        }
    }
}

glib::wrapper! {
    pub struct Stage(ObjectSubclass<imp::Stage>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Stage {
    pub(crate) fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let obj: Self = glib::Object::new();
        obj.set_vexpand(true);
        obj.set_overflow(gtk::Overflow::Visible);
        child.set_parent(&obj);
        obj.imp().child.replace(Some(child.clone().upcast()));
        obj.imp().progress.set(1.);
        obj
    }

    /// Capture immediately before navigation destroys the selected album tile.
    pub(crate) fn set_art_source(
        &self,
        source: &impl IsA<gtk::Widget>,
        path: Option<&std::path::Path>,
    ) {
        let source = path.and_then(|path| {
            self.capture_art(source.as_ref())
                .map(|art| (path.to_owned(), art))
        });
        self.imp().explicit_source.replace(source);
    }

    /// Freeze the currently displayed frame so fast navigation retargets continuously.
    pub(crate) fn prepare(
        &self,
        animate: bool,
        zoom: bool,
        fallback: &gtk::Picture,
        expected: Option<&std::path::Path>,
        fallback_key: Option<&std::path::Path>,
    ) {
        let imp = self.imp();
        if !animate {
            // An artwork/enrichment refresh can replace children during an active transition.
            // Keep its clock and frozen outgoing frame instead of snapping to the destination.
            let target = imp.target.take().and_then(|weak| weak.upgrade());
            if imp.target_key.borrow().as_deref() != expected {
                imp.source.take();
            } else if imp.old.borrow().is_none() {
                imp.source
                    .replace(target.as_ref().and_then(|w| self.capture_art(w)));
            }
            if let Some(target) = target {
                target.set_opacity(1.);
            }
            return;
        }
        let source = imp
            .explicit_source
            .take()
            .filter(|(key, _)| Some(key.as_path()) == expected)
            .map(|(_, art)| art)
            .or_else(|| {
                (expected.is_some() && imp.target_key.borrow().as_deref() == expected)
                    .then(|| {
                        imp.target
                            .borrow()
                            .as_ref()
                            .and_then(glib::WeakRef::upgrade)
                            .and_then(|w| self.capture_art(&w))
                    })
                    .flatten()
            })
            .or_else(|| {
                (expected.is_some() && expected == fallback_key)
                    .then(|| self.capture_art(fallback.upcast_ref()))
                    .flatten()
            });
        self.finish();
        // Even with motion disabled, reuse the sleeve until the large image is ready.
        imp.source.replace(source);
        if !self.is_mapped() || !self.settings().is_gtk_enable_animations() {
            return;
        }
        let Some(frame) = imp.frame.borrow().clone() else {
            return;
        };
        imp.old.replace(Some(frame));
        imp.zoom.set(zoom);
        imp.start.set(0);
        imp.progress.set(0.);
        let tick = self.add_tick_callback(|obj, clock| {
            let imp = obj.imp();
            if !obj.is_mapped() || !obj.settings().is_gtk_enable_animations() {
                obj.finish_frame();
                return glib::ControlFlow::Break;
            }
            if imp.start.get() == 0 {
                imp.start.set(clock.frame_time());
            }
            let p = ((clock.frame_time() - imp.start.get()) as f64 / DURATION_US).clamp(0., 1.);
            imp.progress.set(p);
            obj.queue_draw();
            if p >= 1. {
                obj.finish_frame();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
        imp.tick.replace(Some(tick));
    }

    /// Hide only the destination sleeve; the same artwork is drawn above the moving text.
    pub(crate) fn set_art_target(
        &self,
        target: &impl IsA<gtk::Widget>,
        key: Option<&std::path::Path>,
    ) {
        let imp = self.imp();
        imp.target_key.replace(key.map(std::path::Path::to_owned));
        if let Some(source) = imp.source.borrow().as_ref() {
            if let Some(picture) = target.as_ref().downcast_ref::<gtk::Picture>()
                && picture.paintable().is_none()
            {
                picture.set_paintable(Some(&source.paintable));
            }
            if imp.old.borrow().is_some() {
                target.set_opacity(0.);
            }
        }
        // Retain the geometry so the next transition can start at this sleeve.
        imp.target.replace(Some(target.as_ref().downgrade()));
    }

    fn capture_art(&self, widget: &gtk::Widget) -> Option<Art> {
        if !widget.is_mapped() {
            return None;
        }
        let paintable = widget
            .downcast_ref::<gtk::Picture>()?
            .paintable()?
            .current_image();
        let bounds = picture_bounds(widget, widget.compute_bounds(self)?);
        if bounds.width() <= 0. || bounds.height() <= 0. {
            return None;
        }
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(&snapshot, bounds.width() as f64, bounds.height() as f64);
        Some(Art {
            node: snapshot.to_node()?,
            paintable,
            bounds,
        })
    }

    fn finish_frame(&self) {
        let imp = self.imp();
        if let Some(target) = imp
            .target
            .borrow()
            .as_ref()
            .and_then(glib::WeakRef::upgrade)
        {
            target.set_opacity(1.);
        }
        imp.old.take();
        imp.source.take();
        imp.progress.set(1.);
        imp.tick.take();
        self.queue_draw();
    }

    pub(crate) fn cancel(&self) {
        self.finish();
    }

    fn finish(&self) {
        if let Some(tick) = self.imp().tick.take() {
            tick.remove();
        }
        self.finish_frame();
        self.imp().target.take();
    }
}

fn picture_bounds(widget: &gtk::Widget, rect: graphene::Rect) -> graphene::Rect {
    let Some(paintable) = widget
        .downcast_ref::<gtk::Picture>()
        .and_then(gtk::Picture::paintable)
    else {
        return rect;
    };
    let ratio = paintable.intrinsic_aspect_ratio() as f32;
    if ratio <= 0. {
        return rect;
    }
    let w = rect.width().min(rect.height() * ratio);
    let h = rect.height().min(rect.width() / ratio);
    graphene::Rect::new(
        rect.x() + (rect.width() - w) / 2.,
        rect.y() + (rect.height() - h) / 2.,
        w,
        h,
    )
}

fn ease(p: f64) -> f64 {
    1. - (1. - p).powi(3)
}
fn lerp(a: f32, b: f32, p: f64) -> f32 {
    a + (b - a) * p as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn motion_finishes_exactly_without_overshoot() {
        assert_eq!(ease(0.), 0.);
        assert_eq!(ease(1.), 1.);
        assert_eq!(lerp(52., 295., ease(1.)), 295.);
        let mut last = 0.;
        for i in 0..=100 {
            let p = ease(i as f64 / 100.);
            assert!((last..=1.).contains(&p));
            last = p;
        }
    }
}
