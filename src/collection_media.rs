use super::*;
impl Ui {
    pub(super) fn podcasts(self: &Rc<Self>, list: &gtk::Box) {
        list.append(&app_button(self, "Subscribe to feed", |u| {
            u.prompt("RSS or Atom feed URL", "Subscribe", |u, feed| {
                if !url::Url::parse(&feed)
                    .is_ok_and(|v| matches!(v.scheme(), "http" | "https") && v.has_host())
                {
                    u.notice("Enter an HTTP or HTTPS feed address.");
                    return;
                }
                if !u.db.borrow().feeds.contains(&feed) {
                    u.db.borrow_mut().feeds.push(feed.clone());
                    u.persist();
                }
                u.fetch_feed(feed);
                u.render();
            })
        }));
        for feed in self.db.borrow().feeds.clone() {
            let r = row(8);
            let l = label(&feed, "section");
            l.set_hexpand(true);
            r.append(&l);
            let f = feed.clone();
            r.append(&app_button(self, "Refresh episodes", move |u| {
                u.fetch_feed(f.clone())
            }));
            list.append(&r);
            if let Some(items) = self.feeds.borrow().get(&feed) {
                for item in items {
                    let r = row(6);
                    let l = label(&item.title, "track");
                    l.set_hexpand(true);
                    r.append(&l);
                    let ep = item.clone();
                    r.append(&app_button(self, "Download", move |u| {
                        let tx = u.tx.clone();
                        let ep = ep.clone();
                        u.notice("Downloading episode…");
                        std::thread::spawn(move || {
                            let result = media::download(&ep, &data_dir().join("episodes"));
                            let _ = tx.send(Event::Podcast(ep.title, result));
                        });
                    }));
                    list.append(&r);
                }
            }
        }
        list.append(&label("Downloaded episodes", "section"));
        for (title, path) in self.db.borrow().episodes.clone() {
            list.append(&app_button(self, &title, move |u| {
                u.open_media(&path, true)
            }));
        }
    }
    fn fetch_feed(&self, feed: String) {
        let tx = self.tx.clone();
        self.notice("Reading podcast feed…");
        std::thread::spawn(move || {
            let episodes = media::podcast(&feed, &data_dir().join("feeds"));
            let _ = tx.send(Event::Feed(feed, episodes));
        });
    }
    pub(super) fn show_asset(self: &Rc<Self>, p: &Path) {
        if media::is_photo_path(p) {
            self.slides(vec![p.to_owned()], 0);
        } else {
            self.open_media(p, false);
        }
    }
    fn open_media(self: &Rc<Self>, path: &Path, audio: bool) {
        self.open_embedded(path, audio);
    }
}
