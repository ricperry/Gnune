//! Local QR rendering: integer modules and a four-module quiet zone.
use gtk::prelude::*;
use qrcodegen::{QrCode, QrCodeEcc};

pub(super) fn qr(url: &str) -> Result<gtk::DrawingArea, qrcodegen::DataTooLong> {
    let code = QrCode::encode_text(url, QrCodeEcc::Medium)?;
    let modules = code.size() + 8;
    // At least four logical pixels per module; never scale a bitmap with filtering.
    let size = modules * (280 / modules).max(4);
    let area = gtk::DrawingArea::new();
    area.set_content_width(size);
    area.set_content_height(size);
    area.set_halign(gtk::Align::Start);
    area.set_valign(gtk::Align::Start);
    area.set_accessible_role(gtk::AccessibleRole::Img);
    area.update_property(&[gtk::accessible::Property::Label(
        "Scan this QR code with your phone camera to open the single-use pairing link. Use Copy pairing link as an alternative.",
    )]);
    area.set_draw_func(move |_, cr, width, height| {
        cr.set_antialias(gtk::cairo::Antialias::None);
        cr.set_source_rgb(1., 1., 1.);
        let _ = cr.paint();
        let scale = (width.min(height) / modules).max(1);
        let left = (width - modules * scale) / 2;
        let top = (height - modules * scale) / 2;
        cr.set_source_rgb(0., 0., 0.);
        for y in 0..code.size() {
            for x in 0..code.size() {
                if code.get_module(x, y) {
                    cr.rectangle(
                        f64::from(left + (x + 4) * scale),
                        f64::from(top + (y + 4) * scale),
                        f64::from(scale),
                        f64::from(scale),
                    );
                }
            }
        }
        let _ = cr.fill();
    });
    Ok(area)
}
