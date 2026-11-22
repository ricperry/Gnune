use super::*;
use gtk::{cairo, gdk_pixbuf::Pixbuf};
pub fn palette(path: &Path) -> Option<(u8, u8, u8)> {
    let image = Pixbuf::from_file_at_scale(path, 24, 24, false).ok()?;
    dominant(&image)
}

pub struct ArtSample {
    pub color: Option<(u8, u8, u8)>,
    blur: glib::Bytes,
    alpha: bool,
    stride: i32,
}

/// Both the backdrop and palette come from the same decode on a worker.
pub fn sample(path: &Path) -> Option<ArtSample> {
    let image = Pixbuf::from_file_at_scale(path, 24, 24, false).ok()?;
    let blur = image.scale_simple(5, 5, gtk::gdk_pixbuf::InterpType::Bilinear)?;
    Some(ArtSample {
        color: dominant(&image),
        blur: blur.read_pixel_bytes(),
        alpha: blur.has_alpha(),
        stride: blur.rowstride(),
    })
}

fn dominant(image: &Pixbuf) -> Option<(u8, u8, u8)> {
    let bytes = image.read_pixel_bytes();
    let channels = image.n_channels() as usize;
    // Bucket similar colors together so a single bright pixel cannot recolor the app.
    let mut bins = [[0_u64; 4]; 64];
    for y in 0..image.height() as usize {
        for x in 0..image.width() as usize {
            let i = y * image.rowstride() as usize + x * channels;
            let (r, g, b) = (bytes[i], bytes[i + 1], bytes[i + 2]);
            let alpha = if image.has_alpha() { bytes[i + 3] } else { 255 };
            if alpha < 32 {
                continue;
            }
            let chroma = r.max(g).max(b) - r.min(g).min(b);
            let weight = u64::from(alpha) * (16 + u64::from(chroma));
            let index = usize::from(r >> 6) * 16 + usize::from(g >> 6) * 4 + usize::from(b >> 6);
            let bin = &mut bins[index];
            bin[0] += weight;
            bin[1] += weight * u64::from(r);
            bin[2] += weight * u64::from(g);
            bin[3] += weight * u64::from(b);
        }
    }
    let [weight, r, g, b] = *bins.iter().max_by_key(|bin| bin[0])?;
    (weight != 0).then(|| ((r / weight) as u8, (g / weight) as u8, (b / weight) as u8))
}
pub fn picture(path: Option<&Path>, size: i32) -> gtk::Widget {
    if let Some(path) = path {
        let p = gtk::Picture::new();
        image_loader::load(&p, path, size * p.scale_factor());
        p.set_can_shrink(true);
        p.set_content_fit(gtk::ContentFit::Contain);
        p.set_size_request(size, size);
        p.upcast()
    } else {
        let a = gtk::DrawingArea::new();
        a.set_size_request(size, size);
        a.set_draw_func(|_, cr, w, h| record(cr, w, h));
        a.upcast()
    }
}
fn record(cr: &cairo::Context, w: i32, h: i32) {
    let s = w.min(h) as f64;
    let x = (w as f64 - s) * 0.5;
    let y = (h as f64 - s) * 0.5;
    let grad = cairo::LinearGradient::new(x, y, x + s, y + s);
    grad.add_color_stop_rgb(0., 0.25, 0.16, 0.23);
    grad.add_color_stop_rgb(1., 0.084, 0.07, 0.10);
    cr.set_source(&grad).ok();
    cr.rectangle(x, y, s, s);
    cr.fill().ok();
    let (cx, cy) = (x + s / 2., y + s / 2.);
    let r = s * 0.425;
    let disc = cairo::LinearGradient::new(cx - r, cy - r, cx + r, cy + r);
    for (o, v) in [
        (0., 0.085),
        (0.24, 0.18),
        (0.40, 0.105),
        (0.55, 0.15),
        (0.77, 0.065),
        (1., 0.13),
    ] {
        disc.add_color_stop_rgb(o, v, v * 0.94, v * 1.05);
    }
    cr.set_source(&disc).ok();
    cr.arc(cx, cy, r, 0., std::f64::consts::TAU);
    cr.fill().ok();
    cr.set_source_rgba(0.94, 0.79, 0.86, 0.14);
    cr.set_line_width(s * 0.0013);
    for n in 0..32 {
        cr.arc(
            cx,
            cy,
            r * (0.42 + f64::from(n) * 0.018),
            0.,
            std::f64::consts::TAU,
        );
        cr.stroke().ok();
    }
    cr.set_source_rgb(0.70, 0.42, 0.52);
    cr.arc(cx, cy, r * 0.29, 0., std::f64::consts::TAU);
    cr.fill().ok();
    cr.set_source_rgb(0.13, 0.10, 0.13);
    cr.arc(cx, cy, r * 0.035, 0., std::f64::consts::TAU);
    cr.fill().ok();
}
pub fn backdrop(sample: Option<ArtSample>) -> gtk::Overlay {
    let backdrop = gtk::Overlay::new();
    if let Some(s) = sample {
        let pixels = Pixbuf::from_bytes(
            &s.blur,
            gtk::gdk_pixbuf::Colorspace::Rgb,
            s.alpha,
            8,
            5,
            5,
            s.stride,
        );
        let art = gtk::Picture::for_paintable(&gtk::gdk::Texture::for_pixbuf(&pixels));
        art.set_content_fit(gtk::ContentFit::Fill);
        art.set_can_shrink(true);
        art.set_opacity(0.12);
        backdrop.set_child(Some(&art));
    }
    // A sampled texture and a native gradient scale without rasterizing a window-sized Cairo surface.
    let shade = col(0);
    shade.add_css_class("backdrop-shade");
    backdrop.add_overlay(&shade);
    backdrop.set_hexpand(true);
    backdrop.set_vexpand(true);
    backdrop
}

// Keep the outgoing image through the crossfade, then release it.
pub fn discard_old_backdrops(stack: &gtk::Stack) {
    if stack.is_transition_running() {
        return;
    }
    let current = stack.visible_child();
    let mut next = stack.first_child();
    while let Some(child) = next {
        next = child.next_sibling();
        if current.as_ref() != Some(&child) {
            stack.remove(&child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_dominant_visible_color_and_ignores_transparent_pixels() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cover.png");
        let save = |pixels| {
            Pixbuf::from_bytes(
                &glib::Bytes::from_owned(pixels),
                gtk::gdk_pixbuf::Colorspace::Rgb,
                true,
                8,
                24,
                24,
                24 * 4,
            )
            .savev(&path, "png", &[])
            .unwrap();
        };
        let mut pixels = [255_u8, 0, 0, 0].repeat(24 * 24);
        save(pixels.clone());
        assert_eq!(palette(&path), None);
        for pixel in pixels.chunks_exact_mut(4).take(72) {
            pixel.copy_from_slice(&[30, 120, 100, 255]);
        }
        pixels[..4].copy_from_slice(&[255, 30, 100, 255]);
        save(pixels);
        assert_eq!(palette(&path), Some((30, 120, 100)));
    }
}
