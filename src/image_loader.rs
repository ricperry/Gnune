//! Bounded, CPU-only image work. GTK objects never leave the main thread.
use super::*;
use gtk::gdk_pixbuf::Pixbuf;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

const QUEUE_LIMIT: usize = 96;
const CACHE_BYTES: usize = 32 * 1024 * 1024;
type Work = (Arc<AtomicBool>, Box<dyn FnOnce() + Send>);
struct Pool {
    queue: Mutex<VecDeque<Work>>,
    ready: Condvar,
}
fn pool() -> &'static Arc<Pool> {
    static POOL: OnceLock<Arc<Pool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let pool = Arc::new(Pool {
            queue: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
        });
        for _ in 0..2 {
            let pool = pool.clone();
            std::thread::spawn(move || {
                loop {
                    let (cancel, work) = {
                        let mut queue = pool.queue.lock().unwrap();
                        while queue.is_empty() {
                            queue = pool.ready.wait(queue).unwrap();
                        }
                        queue.pop_back().unwrap()
                    };
                    if !cancel.load(Ordering::Relaxed) {
                        work();
                    }
                }
            });
        }
        pool
    })
}
pub(crate) fn background(cancel: Arc<AtomicBool>, work: impl FnOnce() + Send + 'static) {
    let pool = pool();
    let mut queue = pool.queue.lock().unwrap();
    queue.retain(|(cancel, _)| !cancel.load(Ordering::Relaxed));
    while queue.len() >= QUEUE_LIMIT {
        if let Some((cancel, _)) = queue.pop_front() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
    queue.push_back((cancel, Box::new(work)));
    pool.ready.notify_one();
}
#[derive(Clone)]
struct Pixels {
    bytes: Arc<[u8]>,
    width: i32,
    height: i32,
    stride: i32,
    alpha: bool,
}
#[derive(PartialEq, Eq)]
struct Key {
    path: PathBuf,
    size: i32,
    length: u64,
    modified: Option<std::time::SystemTime>,
}
fn decode(path: &Path, size: i32) -> Option<Pixels> {
    static CACHE: OnceLock<Mutex<VecDeque<(Key, Pixels)>>> = OnceLock::new();
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let key = Key {
        path: path.into(),
        size,
        length: metadata.len(),
        modified: metadata.modified().ok(),
    };
    let cache = CACHE.get_or_init(Default::default);
    {
        let mut cache = cache.lock().unwrap();
        if let Some(i) = cache.iter().position(|(k, _)| *k == key) {
            let found = cache.remove(i).unwrap();
            let pixels = found.1.clone();
            cache.push_back(found);
            return Some(pixels);
        }
    }
    let pixbuf = Pixbuf::from_file_at_scale(path, size, size, true).ok()?;
    let pixbuf = pixbuf.apply_embedded_orientation().unwrap_or(pixbuf);
    let pixels = Pixels {
        bytes: Arc::from(pixbuf.read_pixel_bytes().as_ref()),
        width: pixbuf.width(),
        height: pixbuf.height(),
        stride: pixbuf.rowstride(),
        alpha: pixbuf.has_alpha(),
    };
    let mut cache = cache.lock().unwrap();
    // Keep the cache bounded even when slideshow images are much larger than tiles.
    while cache.iter().map(|(_, p)| p.bytes.len()).sum::<usize>() + pixels.bytes.len() > CACHE_BYTES
    {
        if cache.pop_front().is_none() {
            break;
        }
    }
    if pixels.bytes.len() <= CACHE_BYTES {
        cache.push_back((key, pixels.clone()));
    }
    Some(pixels)
}
struct Pending {
    picture: glib::WeakRef<gtk::Picture>,
    cancel: Arc<AtomicBool>,
    path: PathBuf,
    size: i32,
    submitted: bool,
}
impl Pending {
    fn in_view(&self) -> bool {
        let Some(picture) = self.picture.upgrade().filter(|p| p.is_mapped()) else {
            return false;
        };
        let Some(scroll) = picture.ancestor(gtk::ScrolledWindow::static_type()) else {
            return true;
        };
        let viewport =
            gtk::graphene::Rect::new(0., 0., scroll.width() as f32, scroll.height() as f32);
        picture
            .compute_bounds(&scroll)
            .and_then(|r| r.intersection(&viewport))
            .is_some()
    }
}
struct Loader {
    pending: RefCell<HashMap<u64, Pending>>,
    next: Cell<u64>,
    tx: mpsc::Sender<(u64, Option<Pixels>)>,
    rx: mpsc::Receiver<(u64, Option<Pixels>)>,
    polling: Cell<bool>,
}
thread_local! {
    static LOADER: Rc<Loader> = {
        let (tx, rx) = mpsc::channel();
        Rc::new(Loader { pending: Default::default(), next: Cell::new(0), tx, rx, polling: Cell::new(false) })
    };
}
/// Replaces any prior request for this widget. Decoding and stat calls run on two workers.
pub(crate) fn load(picture: &gtk::Picture, path: &Path, size: i32) {
    picture.set_paintable(None::<&gtk::gdk::Texture>);
    replace_when_ready(picture, path, size);
}

/// Slideshow steps keep the previous frame visible while the next photo decodes.
pub(crate) fn replace_when_ready(picture: &gtk::Picture, path: &Path, size: i32) {
    cancel(picture);
    picture.set_tooltip_text(None);
    LOADER.with(|loader| {
        let id = loader.next.get().wrapping_add(1);
        loader.next.set(id);
        let cancel = Arc::new(AtomicBool::new(false));
        loader.pending.borrow_mut().insert(
            id,
            Pending {
                picture: picture.downgrade(),
                cancel,
                path: path.into(),
                size,
                submitted: false,
            },
        );
        if loader.polling.replace(true) {
            return;
        }
        let loader = loader.clone();
        glib::timeout_add_local(Duration::from_millis(16), move || {
            loader.pending.borrow_mut().retain(|_, p| {
                if p.picture.upgrade().is_none() {
                    p.cancel.store(true, Ordering::Relaxed);
                    return false;
                }
                // An evicted job is retried; visible tiles never silently lose their request.
                if p.cancel.load(Ordering::Relaxed) {
                    p.cancel = Arc::new(AtomicBool::new(false));
                    p.submitted = false;
                }
                true
            });
            for _ in 0..12 {
                let Ok((id, pixels)) = loader.rx.try_recv() else {
                    break;
                };
                let Some(pending) = loader.pending.borrow_mut().remove(&id) else {
                    continue;
                };
                if let Some(picture) = pending.picture.upgrade() {
                    if let Some(p) = pixels {
                        let pixbuf = Pixbuf::from_bytes(
                            &glib::Bytes::from_owned(p.bytes),
                            gtk::gdk_pixbuf::Colorspace::Rgb,
                            p.alpha,
                            8,
                            p.width,
                            p.height,
                            p.stride,
                        );
                        picture.set_paintable(Some(&gtk::gdk::Texture::for_pixbuf(&pixbuf)));
                        picture.set_tooltip_text(None);
                    } else {
                        let icon = gtk::IconTheme::for_display(&picture.display()).lookup_icon(
                            "image-missing-symbolic",
                            &[],
                            32,
                            picture.scale_factor(),
                            gtk::TextDirection::None,
                            gtk::IconLookupFlags::empty(),
                        );
                        picture.set_paintable(Some(&icon));
                        picture.set_tooltip_text(Some("This picture could not be decoded."));
                    }
                }
            }
            let active = loader
                .pending
                .borrow()
                .values()
                .filter(|p| p.submitted)
                .count();
            if active < 8 {
                let mut waiting: Vec<_> = loader
                    .pending
                    .borrow()
                    .iter()
                    .filter(|(_, p)| !p.submitted)
                    .map(|(id, p)| (p.in_view(), *id))
                    .collect();
                waiting.sort_unstable_by(|a, b| b.cmp(a));
                for (_, id) in waiting.into_iter().take(8_usize.saturating_sub(active)) {
                    let mut pending = loader.pending.borrow_mut();
                    let p = pending.get_mut(&id).unwrap();
                    p.submitted = true;
                    let path = p.path.clone();
                    let size = p.size;
                    let cancelled = p.cancel.clone();
                    let tx = loader.tx.clone();
                    background(p.cancel.clone(), move || {
                        let pixels = decode(&path, size.clamp(1, 2560));
                        if !cancelled.load(Ordering::Relaxed) {
                            let _ = tx.send((id, pixels));
                        }
                    });
                }
            }
            if loader.pending.borrow().is_empty() {
                loader.polling.set(false);
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    });
}
pub(crate) fn cancel(picture: &gtk::Picture) {
    LOADER.with(|loader| {
        loader.pending.borrow_mut().retain(|_, p| {
            let keep = p.picture.upgrade().is_some_and(|other| other != *picture);
            if !keep {
                p.cancel.store(true, Ordering::Relaxed);
            }
            keep
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn worker_decode_bounds_output_and_never_changes_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.xpm");
        let mut fixture = String::from(
            "/* XPM */\nstatic char *image[] = {\n\"1200 800 1 1\",\n\"a c #3696aa\",\n",
        );
        for row in 0..800 {
            fixture.push('"');
            fixture.push_str(&"a".repeat(1200));
            fixture.push_str(if row == 799 { "\"};\n" } else { "\",\n" });
        }
        std::fs::write(&path, fixture).unwrap();
        let before = std::fs::read(&path).unwrap();
        let pixels = decode(&path, 160).unwrap();
        assert_eq!(pixels.width, 160);
        assert!((106..=107).contains(&pixels.height));
        assert!(pixels.bytes.len() < 160 * 160 * 4);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        #[cfg(unix)]
        {
            let link = dir.path().join("linked.xpm");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            let linked = decode(&link, 160).unwrap();
            assert_eq!(linked.bytes, pixels.bytes);
            assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        }
    }
    #[test]
    fn unreadable_and_unsupported_images_return_a_placeholder_result() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.jpg");
        std::fs::write(&path, b"not an image").unwrap();
        assert!(decode(&path, 120).is_none());
        assert!(decode(&dir.path().join("missing.jpg"), 120).is_none());
        assert!(decode(dir.path(), 120).is_none());
    }
}
