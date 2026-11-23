//! GTK-native video frames and shared transport, using GStreamer's maintained binding.
use super::*;
use gnune::player::PlaybackState;
use gstreamer::{self as gst, prelude::*};

#[derive(Clone)]
struct VideoPlaylist {
    items: Rc<[PathBuf]>,
    index: usize,
}

impl VideoPlaylist {
    fn step(&self, delta: isize) -> Option<Self> {
        let index = self.index.checked_add_signed(delta)?;
        (index < self.items.len()).then(|| Self {
            items: self.items.clone(),
            index,
        })
    }
}

#[derive(Clone)]
struct PodcastSequence {
    feed: String,
    episodes: Rc<[media::Episode]>,
    index: usize,
}
impl PodcastSequence {
    fn step(&self, delta: isize) -> Option<Self> {
        let index = self.index.checked_add_signed(delta)?;
        (index < self.episodes.len()).then(|| Self {
            feed: self.feed.clone(),
            episodes: self.episodes.clone(),
            index,
        })
    }
}

pub(super) struct Media {
    pipeline: gst::Element,
    paintable: gtk::gdk::Paintable,
    pub path: PathBuf,
    pub title: String,
    pub stream_uri: Option<String>,
    pub show_feed: Option<String>,
    pub podcast_id: Option<podcasts::EpisodeId>,
    podcast_sequence: Option<PodcastSequence>,
    requested_seek: Cell<Option<(f64, Instant)>>,
    captured_progress: Cell<Option<podcasts::EpisodeProgress>>,
    stream_artist: String,
    stream_album: String,
    artwork: MediaArtwork,
    pub audio: bool,
    playlist: Option<VideoPlaylist>,
    pub return_page: Page,
    paused: Cell<bool>,
    buffering: Cell<bool>,
    ended: Cell<bool>,
    completion_seen: Cell<bool>,
    error: RefCell<Option<String>>,
    resume: Cell<Option<f64>>,
    rate: Cell<f64>,
    rate_picker: glib::WeakRef<gtk::DropDown>,
}

impl Media {
    fn new(
        path: &Path,
        title: String,
        audio: bool,
        return_page: Page,
        volume: f64,
        resume: f64,
        stream_uri: Option<&str>,
    ) -> Result<Self, String> {
        gst::init().map_err(|e| e.to_string())?;
        let pipeline = gst::ElementFactory::make("playbin")
            .build()
            .map_err(|e| e.to_string())?;
        let sink = gst::ElementFactory::make("gtk4paintablesink")
            .build()
            .map_err(|_| {
                "Install gstreamer1-plugin-gtk4 for embedded video playback.".to_string()
            })?;
        let paintable = sink.property::<gtk::gdk::Paintable>("paintable");
        pipeline.set_property("video-sink", &sink);
        if std::env::var_os("GNUNE_AUDIO_NULL").is_some_and(|v| v != "0") {
            let sink = gst::ElementFactory::make("fakesink")
                .property("sync", true)
                .build()
                .map_err(|e| e.to_string())?;
            pipeline.set_property("audio-sink", &sink);
        }
        if let Some(uri) = stream_uri {
            pipeline.set_property("uri", uri);
            // Streaming buffers stay in memory; never enable playbin's disk download mode.
            let flags = pipeline.property_value("flags");
            if let Some(class) = glib::FlagsClass::with_type(flags.type_())
                && let Ok(flags) = class.unset_by_nick(flags, "download")
            {
                pipeline.set_property_from_value("flags", &flags);
            }
        } else {
            pipeline.set_property("uri", gio::File::for_path(path).uri().as_str());
        }
        pipeline.set_property("volume", volume.clamp(0., 100.) / 100.);
        if stream_uri.is_none()
            && let Some(subtitle) = subtitle_sidecar(path)
        {
            pipeline.set_property("suburi", gio::File::for_path(subtitle).uri().as_str());
        }
        pipeline
            .set_state(gst::State::Paused)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            pipeline,
            paintable,
            path: path.into(),
            title,
            stream_uri: stream_uri.map(str::to_owned),
            show_feed: None,
            podcast_id: None,
            podcast_sequence: None,
            requested_seek: Cell::new(None),
            captured_progress: Cell::new(None),
            stream_artist: String::new(),
            stream_album: String::new(),
            artwork: MediaArtwork::default(),
            playlist: None,
            audio,
            return_page,
            paused: Cell::new(false),
            buffering: Cell::new(false),
            ended: Cell::new(false),
            completion_seen: Cell::new(false),
            error: RefCell::new(None),
            resume: Cell::new((resume > 0.).then_some(resume)),
            rate: Cell::new(1.),
            rate_picker: glib::WeakRef::new(),
        })
    }

    fn snapshot(&self) -> PlaybackState {
        if let Some(bus) = self.pipeline.bus() {
            for message in bus.iter_filtered(&[
                gst::MessageType::Error,
                gst::MessageType::Eos,
                gst::MessageType::Buffering,
            ]) {
                match message.view() {
                    gst::MessageView::Error(e) => {
                        *self.error.borrow_mut() = Some(e.error().to_string());
                        self.paused.set(true);
                        let _ = self.pipeline.set_state(gst::State::Paused);
                    }
                    gst::MessageView::Buffering(message) if self.stream_uri.is_some() => {
                        let buffering = message.percent() < 100;
                        self.buffering.set(buffering);
                        let state = if buffering || self.paused.get() {
                            gst::State::Paused
                        } else {
                            gst::State::Playing
                        };
                        let _ = self.pipeline.set_state(state);
                    }
                    gst::MessageView::Eos(_) => {
                        self.ended.set(true);
                        self.paused.set(true);
                    }
                    _ => {}
                }
            }
        }
        let duration = self
            .pipeline
            .query_duration::<gst::ClockTime>()
            .map_or(0., |v| v.nseconds() as f64 / 1e9);
        if duration > 0.
            && let Some(resume) = self.resume.get()
            && self.seek(resume.min((duration - 0.1).max(0.))).is_ok()
        {
            self.resume.set(None);
        }
        let queried = self
            .pipeline
            .query_position::<gst::ClockTime>()
            .map_or(0., |value| value.nseconds() as f64 / 1e9);
        let position = if let Some((target, requested)) = self.requested_seek.get() {
            if requested.elapsed() < Duration::from_secs(2) && (queried - target).abs() > 2. {
                target
            } else {
                self.requested_seek.set(None);
                queried
            }
        } else {
            queried
        };
        PlaybackState {
            track_id: Some(self.path.to_string_lossy().into_owned()),
            queue_index: None,
            paused: self.paused.get(),
            finished: self.ended.get(),
            loaded: duration > 0.,
            duration,
            position,
            position_updated: std::time::Instant::now(),
            volume: self.pipeline.property::<f64>("volume") * 100.,
            error: self.error.borrow().clone(),
        }
    }

    fn set_paused(&self, paused: bool) -> Result<(), String> {
        if !paused && self.ended.get() {
            self.seek(0.)?;
        }
        self.pipeline
            .set_state(if paused || self.buffering.get() {
                gst::State::Paused
            } else {
                gst::State::Playing
            })
            .map_err(|e| e.to_string())?;
        self.paused.set(paused);
        Ok(())
    }

    fn seek(&self, seconds: f64) -> Result<(), String> {
        if !seconds.is_finite() {
            return Err("Invalid playback position".into());
        }
        self.pipeline
            .seek(
                self.rate.get(),
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::SeekType::Set,
                gst::ClockTime::from_nseconds((seconds.max(0.) * 1e9) as u64),
                gst::SeekType::None,
                gst::ClockTime::NONE,
            )
            .map_err(|e| e.to_string())?;
        self.ended.set(false);
        self.completion_seen.set(false);
        self.requested_seek
            .set(Some((seconds.max(0.), Instant::now())));
        Ok(())
    }

    fn set_rate(&self, rate: f64) -> Result<(), String> {
        if !rate.is_finite() || !(0.25..=4.).contains(&rate) {
            return Err("Choose a speed between 0.25× and 4×".into());
        }
        let old = self.rate.replace(rate);
        let position = self
            .pipeline
            .query_position::<gst::ClockTime>()
            .map_or(0., |v| v.nseconds() as f64 / 1e9);
        if let Err(e) = self.seek(position) {
            self.rate.set(old);
            return Err(e);
        }
        Ok(())
    }

    pub(super) fn cycle_subtitles(&self) -> String {
        let count = self.pipeline.property::<i32>("n-text");
        if count == 0 {
            return "No subtitle tracks. Matching .srt/.vtt/.ass/.ssa sidecars load automatically."
                .into();
        }
        let current = self.pipeline.property::<i32>("current-text");
        // -1 is automatic, so use the playbin text flag to provide a real off state.
        let flags = self.pipeline.property_value("flags");
        let class = glib::FlagsClass::with_type(flags.type_()).expect("GstPlayFlags");
        let is_enabled = class.is_set_by_nick(&flags, "text");
        if is_enabled && current + 1 >= count {
            self.pipeline.set_property_from_value(
                "flags",
                &class.unset_by_nick(flags, "text").expect("text flag"),
            );
            "Subtitles off".into()
        } else {
            let next = if is_enabled { (current + 1).max(0) } else { 0 };
            if !is_enabled {
                self.pipeline.set_property_from_value(
                    "flags",
                    &class.set_by_nick(flags, "text").expect("text flag"),
                );
            }
            self.pipeline.set_property("current-text", next);
            format!("Subtitle track {} of {count}", next + 1)
        }
    }
}

impl Drop for Media {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

impl Ui {
    pub(super) fn open_embedded(self: &Rc<Self>, path: &Path, audio: bool) {
        let saved = self.db.borrow().podcast_downloads.get(path).cloned();
        if let Some(id) = saved {
            let episode = self
                .feeds
                .borrow()
                .get(&id.feed_url)
                .and_then(|episodes| {
                    episodes
                        .iter()
                        .find(|episode| episode.url == id.episode_url)
                })
                .cloned()
                .unwrap_or_else(|| media::Episode {
                    title: self.video_title(path),
                    url: id.episode_url.clone(),
                    ..Default::default()
                });
            self.stream_podcast(&id.feed_url, &episode);
            return;
        }
        self.command(self.select_embedded(path, audio, None, true));
    }

    pub(super) fn stream_podcast(self: &Rc<Self>, feed: &str, episode: &media::Episode) {
        self.podcast_latest_pending.borrow_mut().take();
        self.podcast_resume_pending.borrow_mut().take();
        let result = self.select_podcast_stream(feed, episode, None, true);
        self.command(result);
    }

    fn select_podcast_stream(
        self: &Rc<Self>,
        feed: &str,
        episode: &media::Episode,
        start: Option<f64>,
        reveal: bool,
    ) -> Result<(), String> {
        let id = podcasts::EpisodeId::new(feed, &episode.url)?;
        let uri = id.episode_url.clone();
        if self
            .media
            .borrow()
            .as_ref()
            .is_some_and(|media| media.podcast_id.as_ref() == Some(&id))
        {
            if let Some(position) = start {
                self.seek_playback(position)?;
            }
            self.set_playback_paused(false)?;
            let state = self.playback_state();
            self.db
                .borrow_mut()
                .record_podcast_progress(&id, state.position, false);
            self.persist_playback();
            if reveal {
                self.go(Page::PodcastNow);
            }
            return Ok(());
        }
        let show = self
            .db
            .borrow()
            .podcast_shows
            .get(feed)
            .cloned()
            .unwrap_or_default();
        // This key identifies progress only. It is never handed to a file/media tool.
        let identity = PathBuf::from(format!("podcast:{}", enrichment::key(&[&uri])));
        let saved = self
            .db
            .borrow()
            .podcast_progress
            .get(&id.feed_url)
            .and_then(|episodes| episodes.get(&id.episode_url))
            .copied();
        let resume = start.unwrap_or_else(|| {
            saved.map_or_else(
                || {
                    self.db
                        .borrow()
                        .progress
                        .get(&identity)
                        .copied()
                        .unwrap_or(0.)
                },
                |saved| {
                    if saved.completed {
                        0.
                    } else {
                        saved.position_secs.max(0.)
                    }
                },
            )
        });
        let local = self
            .db
            .borrow()
            .podcast_downloads
            .iter()
            .find(|(path, candidate)| *candidate == &id && path.is_file())
            .map(|(path, _)| path.clone());
        self.save_position();
        let mut media = Media::new(
            local.as_deref().unwrap_or(&identity),
            episode.title.clone(),
            true,
            Page::Podcast(feed.into()),
            self.db.borrow().volume.unwrap_or(80.),
            resume,
            local.is_none().then_some(uri.as_str()),
        )?;
        media.podcast_id = Some(id.clone());
        media.captured_progress.set(Some(podcasts::EpisodeProgress {
            position_secs: resume,
            completed: false,
        }));
        let order = self
            .db
            .borrow()
            .podcast_orders
            .get(feed)
            .copied()
            .unwrap_or_default();
        let mut episodes = podcasts::ordered_episodes(
            self.feeds.borrow().get(feed).map_or(&[], Vec::as_slice),
            order,
        );
        if !episodes
            .iter()
            .any(|candidate| candidate.url == episode.url)
        {
            episodes.push(episode.clone());
        }
        let index = episodes
            .iter()
            .position(|candidate| candidate.url == episode.url)
            .unwrap_or(0);
        media.podcast_sequence = Some(PodcastSequence {
            feed: feed.into(),
            episodes: episodes.into(),
            index,
        });
        media.show_feed = Some(feed.into());
        media.stream_artist = show.publisher;
        media.stream_album = show.title;
        media.artwork.cover = show
            .image
            .as_deref()
            .map(media::podcast_cover_path)
            .filter(|path| path.is_file());
        self.close_embedded();
        self.player.set_paused(true)?;
        let started = media.set_paused(false);
        *self.media.borrow_mut() = Some(media);
        self.db
            .borrow_mut()
            .record_podcast_progress(&id, resume, false);
        self.persist_playback();
        self.next.set_sensitive(self.has_next_video());
        self.notice("");
        self.update_cover();
        if reveal {
            self.go(Page::PodcastNow);
        } else if *self.page.borrow() == Page::PodcastNow {
            self.render();
        }
        started
    }

    pub(super) fn play_video_playlist(self: &Rc<Self>, items: Rc<[PathBuf]>, index: usize) {
        let playlist = VideoPlaylist { items, index };
        self.command(self.select_playlist_item(playlist, true));
    }

    fn select_playlist_item(
        self: &Rc<Self>,
        playlist: VideoPlaylist,
        reveal: bool,
    ) -> Result<(), String> {
        let path = playlist
            .items
            .get(playlist.index)
            .ok_or("This playlist has no video at that position.")?
            .clone();
        self.select_embedded(&path, false, Some(playlist), reveal)
    }

    fn select_embedded(
        self: &Rc<Self>,
        path: &Path,
        audio: bool,
        playlist: Option<VideoPlaylist>,
        reveal: bool,
    ) -> Result<(), String> {
        self.podcast_latest_pending.borrow_mut().take();
        // Reopening the visible item retains its running pipeline and playhead.
        if reveal
            && self
                .media
                .borrow()
                .as_ref()
                .is_some_and(|m| m.path == path && m.audio == audio)
        {
            if let Some(playlist) = playlist {
                let mut active = self.media.borrow_mut();
                let media = active.as_mut().unwrap();
                media.playlist = Some(playlist);
                if media.ended.get() {
                    media.set_paused(false)?;
                }
            }
            self.next.set_sensitive(self.has_next_video());
            self.go(Page::Video);
            return Ok(());
        }
        self.save_position();
        let prior = if !reveal || *self.page.borrow() == Page::Video {
            self.media
                .borrow()
                .as_ref()
                .map_or(Page::Now, |m| m.return_page.clone())
        } else {
            self.page.borrow().clone()
        };
        let volume = self.db.borrow().volume.unwrap_or(80.);
        let resume = self.db.borrow().progress.get(path).copied().unwrap_or(0.);
        let mut media = Media::new(
            path,
            self.video_title(path),
            audio,
            prior,
            volume,
            resume,
            None,
        )?;
        media.artwork = self.embedded_artwork(path);
        media.playlist = playlist;
        self.close_embedded();
        self.command(self.player.set_paused(true));
        let started = media.set_paused(false);
        *self.media.borrow_mut() = Some(media);
        self.next.set_sensitive(self.has_next_video());
        self.notice("");
        if reveal {
            self.go(Page::Video);
        } else if *self.page.borrow() == Page::Video {
            self.render();
        } else {
            self.update_cover();
        }
        started
    }

    pub(super) fn has_next_video(&self) -> bool {
        self.media.borrow().as_ref().is_some_and(|media| {
            media
                .podcast_sequence
                .as_ref()
                .is_some_and(|sequence| sequence.index + 1 < sequence.episodes.len())
                || media
                    .playlist
                    .as_ref()
                    .is_some_and(|sequence| sequence.index + 1 < sequence.items.len())
        })
    }
    fn adjacent_podcast(&self, delta: isize, skip_completed: bool) -> Option<PodcastSequence> {
        let mut sequence = self.media.borrow().as_ref()?.podcast_sequence.clone()?;
        loop {
            sequence = sequence.step(delta)?;
            if !skip_completed
                || !self
                    .db
                    .borrow()
                    .podcast_progress
                    .get(&sequence.feed)
                    .and_then(|progress| progress.get(&sequence.episodes[sequence.index].url))
                    .is_some_and(|saved| saved.completed)
            {
                return Some(sequence);
            }
        }
    }
    fn play_podcast_sequence(self: &Rc<Self>, sequence: PodcastSequence) -> Result<(), String> {
        self.select_podcast_stream(
            &sequence.feed,
            &sequence.episodes[sequence.index],
            Some(0.),
            false,
        )
    }
    pub(super) fn reorder_playing_podcast(&self, feed: &str) {
        let order = self
            .db
            .borrow()
            .podcast_orders
            .get(feed)
            .copied()
            .unwrap_or_default();
        let episodes = podcasts::ordered_episodes(
            self.feeds.borrow().get(feed).map_or(&[], Vec::as_slice),
            order,
        );
        if let Some(media) = self.media.borrow_mut().as_mut()
            && media.show_feed.as_deref() == Some(feed)
            && let Some(id) = &media.podcast_id
            && let Some(index) = episodes
                .iter()
                .position(|episode| episode.url == id.episode_url)
        {
            media.podcast_sequence = Some(PodcastSequence {
                feed: feed.into(),
                episodes: episodes.into(),
                index,
            });
        }
        self.next.set_sensitive(self.has_next_video());
    }
    pub(super) fn capture_podcast_progress(&self, state: &PlaybackState, position: Option<f64>) {
        let media = self.media.borrow();
        if let Some(media) = media.as_ref()
            && let Some(id) = &media.podcast_id
            && media.resume.get().is_none()
            && let Some(saved) = podcasts::progress_update(
                media.captured_progress.get(),
                position.unwrap_or(state.position),
                state.finished,
                state.paused,
                position.is_some(),
            )
        {
            self.db
                .borrow_mut()
                .record_podcast_progress(id, saved.position_secs, saved.completed);
            media.captured_progress.set(Some(saved));
            self.podcast_progress_dirty.set(true);
        }
    }
    pub(super) fn resume_podcast_episode(
        self: &Rc<Self>,
        feed: &str,
        episode: &media::Episode,
        position: f64,
    ) {
        self.command(self.select_podcast_stream(feed, episode, Some(position), true));
    }
    pub(super) fn force_latest_podcast(self: &Rc<Self>, feed: &str, episode: &media::Episode) {
        self.podcast_latest_pending.borrow_mut().take();
        self.podcast_resume_pending.borrow_mut().take();
        self.command(self.select_podcast_stream(feed, episode, Some(0.), true));
    }
    fn adjacent_video(&self, delta: isize) -> Option<VideoPlaylist> {
        self.media.borrow().as_ref()?.playlist.as_ref()?.step(delta)
    }

    // Handle an EOS once, including when opening its successor fails.
    pub(super) fn advance_finished_video(self: &Rc<Self>) -> bool {
        let podcast = self.media.borrow().as_ref().and_then(|media| {
            media
                .show_feed
                .clone()
                .map(|feed| (feed, media.completion_seen.replace(true)))
        });
        if let Some((_, true)) = podcast {
            return false;
        }
        if let Some(next) = self.adjacent_podcast(1, true) {
            self.command(self.play_podcast_sequence(next));
            return true;
        }
        if let Some((feed, _)) = podcast {
            self.refresh_podcast_views(&feed);
            return false;
        }
        let Some(next) = self.adjacent_video(1) else {
            return false;
        };
        let result = self.select_playlist_item(next, false);
        if let Err(message) = result {
            if let Some(active) = self.media.borrow_mut().as_mut() {
                active.playlist = None;
            }
            self.next.set_sensitive(false);
            self.notice(&message);
        }
        true
    }

    pub(super) fn close_embedded(&self) {
        let media = self.media.borrow_mut().take();
        if let Some(media) = media {
            let state = media.snapshot();
            if let Some(id) = &media.podcast_id
                && media.resume.get().is_none()
                && let Some(saved) = podcasts::progress_update(
                    media.captured_progress.get(),
                    state.position,
                    state.finished,
                    state.paused,
                    false,
                )
            {
                self.db.borrow_mut().record_podcast_progress(
                    id,
                    saved.position_secs,
                    saved.completed,
                );
            }
            self.db.borrow_mut().progress.insert(
                media.path.clone(),
                if state.finished { 0. } else { state.position },
            );
            drop(media);
            let music = self.player.snapshot();
            self.pos.set(music.position);
            self.duration.set(music.duration);
            if let Some(track) = self.current.borrow().as_ref() {
                self.mini_title
                    .set_text(&format!("{} · {}", track.artist, track.title));
                self.mini_title
                    .set_tooltip_text(Some(&format!("{} · {}", track.artist, track.album)));
            } else {
                self.mini_title.set_text("Nothing playing");
                self.mini_title.set_tooltip_text(None);
            }
            self.next.set_sensitive(true);
            self.update_cover();
            self.persist();
        }
    }

    pub(super) fn embedded_view(self: &Rc<Self>) {
        let media = self.media.borrow();
        let Some(media) = media.as_ref() else { return };
        let root = col(8);
        root.add_css_class("embedded-media");
        root.set_vexpand(true);
        let actions = row(8);
        actions.set_margin_start(22);
        actions.set_margin_end(22);
        let prior = media.return_page.clone();
        let return_label = match &prior {
            Page::Collection(_) => "← return to collection",
            Page::Quick => "← return to quickplay",
            _ => "← return",
        };
        actions.append(&app_button(self, return_label, move |u| {
            u.go(prior.clone())
        }));
        let names = col(0);
        names.set_hexpand(true);
        names.set_valign(gtk::Align::Center);
        names.append(&label(&media.title, "video-title"));
        if let Some(tags) = self.db.borrow().video_tags.get(&media.path) {
            let caption = tags.song().credit();
            if !caption.is_empty() {
                names.append(&label(&caption, "small"));
            }
        }
        actions.append(&names);
        let path = media.path.clone();
        if !media.audio {
            actions.append(&app_button(self, "Song info", move |u| u.video_info(&path)));
        }
        let rates: Vec<_> = (1..=16).map(|n| format!("{}×", n as f64 / 4.)).collect();
        let speed =
            gtk::DropDown::from_strings(&rates.iter().map(String::as_str).collect::<Vec<_>>());
        speed.set_selected((media.rate.get() * 4. - 1.).round() as u32);
        media.rate_picker.set(Some(&speed));
        speed.set_tooltip_text(Some("Playback speed"));
        let u = self.clone();
        speed.connect_selected_notify(move |s| {
            let rate = (s.selected() + 1) as f64 / 4.;
            if (u.playback_rate() - rate).abs() > 0.01 {
                u.command(u.set_playback_rate(rate));
            }
        });
        actions.append(&speed);
        actions.append(&app_button(self, "Subtitles", |u| {
            if let Some(media) = u.media.borrow().as_ref() {
                u.notice(&media.cycle_subtitles());
            }
        }));
        actions.append(&app_button(self, "Full screen", |u| u.fullscreen()));
        actions.append(&app_button(self, "Stop", |u| u.command(u.stop_playback())));
        self.nav
            .bind_property("visible", &actions, "visible")
            .sync_create()
            .build();
        root.append(&actions);
        if media.audio {
            let image = gtk::Image::from_icon_name("audio-x-generic-symbolic");
            image.set_pixel_size(128);
            image.set_vexpand(true);
            root.append(&image);
        } else {
            let picture = gtk::Picture::for_paintable(&media.paintable);
            picture.add_css_class("video-frame");
            picture.set_content_fit(gtk::ContentFit::Contain);
            picture.set_can_shrink(true);
            picture.set_hexpand(true);
            picture.set_vexpand(true);
            root.append(&picture);
        }
        self.body.append(&root);
    }

    /// A still image representing the active media, also suitable for palette extraction.
    pub(super) fn playback_artwork(&self) -> Option<PathBuf> {
        if let Some(media) = self.media.borrow().as_ref() {
            return media.artwork.cover.clone();
        }
        let track = self.current.borrow().clone()?;
        self.cover(&track)
            .filter(|p| p.is_file())
            .or_else(|| self.artist_portrait(&track.artist))
    }

    pub(super) fn artist_portrait(&self, artist: &str) -> Option<PathBuf> {
        let artist = normalized_credit(artist);
        if artist.is_empty() {
            return None;
        }
        self.db
            .borrow()
            .portraits
            .iter()
            .find(|(name, path)| normalized_credit(name) == artist && path.is_file())
            .map(|(_, path)| path.clone())
    }

    pub(super) fn refresh_embedded_artwork(&self) {
        if let Some(media) = self.media.borrow_mut().as_mut() {
            if let Some(feed) = &media.show_feed {
                media.artwork.cover = self
                    .db
                    .borrow()
                    .podcast_shows
                    .get(feed)
                    .and_then(|show| show.image.as_deref())
                    .map(media::podcast_cover_path)
                    .filter(|path| path.is_file());
            } else {
                media.artwork = self.embedded_artwork(&media.path);
            }
        }
    }

    fn embedded_artwork(&self, path: &Path) -> MediaArtwork {
        let song = self
            .db
            .borrow()
            .video_tags
            .get(path)
            .map(|v| v.song().clone())
            .unwrap_or_default();
        let sidecars = media_sidecars(path);
        let portrait = self.artist_portrait(&song.artist).or(sidecars.portrait);
        let db = self.db.borrow();
        let mut tracks: Vec<_> = db
            .tracks
            .iter()
            .filter(|t| artist_matches(&song.artist, t))
            .filter(|t| same_credit(&song.album, &t.album) || same_credit(&song.title, &t.title))
            .collect();
        tracks.sort_by_key(|t| {
            std::cmp::Reverse((
                same_credit(&song.album, &t.album),
                same_credit(&song.title, &t.title),
            ))
        });
        let cover = tracks
            .into_iter()
            .find_map(|t| self.cover(t).filter(|p| p.is_file()))
            .or(sidecars.cover)
            .or_else(|| portrait.clone())
            .or_else(|| {
                let poster = media::poster_path(path, &data_dir().join("posters"));
                poster.is_file().then_some(poster)
            });
        MediaArtwork { portrait, cover }
    }

    /// Refresh only the shared footer picture; browsing uses the active sink paintable.
    pub(super) fn update_thumbnail(&self) {
        let media = self.media.borrow();
        if let Some(media) = media.as_ref()
            && !media.audio
            && (*self.page.borrow() != Page::Video || self.mini.get())
        {
            self.thumb_art.take();
            image_loader::cancel(&self.thumb);
            self.thumb.set_paintable(Some(&media.paintable));
            return;
        }
        let still = media
            .as_ref()
            .and_then(|m| m.artwork.portrait.clone())
            .or_else(|| self.playback_artwork());
        if let Some(path) = still {
            if self.thumb_art.borrow().as_ref() != Some(&path) {
                image_loader::load(&self.thumb, &path, 64 * self.thumb.scale_factor());
                self.thumb_art.replace(Some(path));
            }
        } else {
            self.thumb_art.take();
            image_loader::cancel(&self.thumb);
            let name = if media.as_ref().is_some_and(|m| !m.audio) {
                "video-x-generic-symbolic"
            } else {
                "audio-x-generic-symbolic"
            };
            let icon = gtk::IconTheme::for_display(&self.thumb.display()).lookup_icon(
                name,
                &[],
                36,
                self.thumb.scale_factor(),
                gtk::TextDirection::None,
                gtk::IconLookupFlags::empty(),
            );
            self.thumb.set_paintable(Some(&icon));
        }
    }

    pub(super) fn stop_playback(self: &Rc<Self>) -> Result<(), String> {
        let prior = self
            .media
            .borrow()
            .as_ref()
            .map(|m| (m.return_page.clone(), m.path.clone()));
        if let Some((prior, path)) = prior {
            // An explicit stop releases the video sink and leaves music paused.
            self.close_embedded();
            self.notice("");
            self.db.borrow_mut().progress.insert(path, 0.);
            self.persist();
            self.player.set_paused(true)?;
            if matches!(*self.page.borrow(), Page::Video | Page::PodcastNow) {
                self.go(prior);
            } else {
                self.render();
            }
            Ok(())
        } else {
            self.set_playback_paused(true)?;
            self.seek_playback(0.)
        }
    }

    pub(super) fn playback_state(&self) -> PlaybackState {
        self.media
            .borrow()
            .as_ref()
            .map_or_else(|| self.player.snapshot(), Media::snapshot)
    }
    pub(super) fn set_playback_paused(&self, paused: bool) -> Result<(), String> {
        self.media
            .borrow()
            .as_ref()
            .map_or_else(|| self.player.set_paused(paused), |m| m.set_paused(paused))?;
        self.playhead
            .borrow_mut()
            .pause(paused, glib::monotonic_time());
        self.draw_playhead(glib::monotonic_time());
        let state = self.playback_state();
        self.capture_podcast_progress(&state, Some(state.position));
        self.podcast_progress_dirty.set(false);
        self.persist_playback();
        Ok(())
    }
    pub(super) fn seek_playback(&self, seconds: f64) -> Result<(), String> {
        self.media
            .borrow()
            .as_ref()
            .map_or_else(|| self.player.seek(seconds), |m| m.seek(seconds))?;
        self.playhead
            .borrow_mut()
            .seek(seconds, glib::monotonic_time());
        self.draw_playhead(glib::monotonic_time());
        self.capture_podcast_progress(&self.playback_state(), Some(seconds.max(0.)));
        self.persist_playback();
        Ok(())
    }
    pub(super) fn set_playback_volume(&self, volume: f64) -> Result<(), String> {
        if let Some(media) = self.media.borrow().as_ref() {
            media
                .pipeline
                .set_property("volume", volume.clamp(0., 100.) / 100.);
        }
        self.player.set_volume(volume)
    }
    pub(super) fn next_playback(self: &Rc<Self>) -> Result<(), String> {
        if let Some(next) = self.adjacent_podcast(1, false) {
            return self.play_podcast_sequence(next);
        }
        if let Some(next) = self.adjacent_video(1) {
            self.select_playlist_item(next, false)
        } else if self.media.borrow().is_some() {
            Ok(())
        } else {
            self.player.next()
        }
    }
    pub(super) fn previous_playback(self: &Rc<Self>) -> Result<(), String> {
        if let Some(previous) = self.adjacent_podcast(-1, false) {
            return self.play_podcast_sequence(previous);
        }
        if let Some(previous) = self.adjacent_video(-1) {
            self.select_playlist_item(previous, false)
        } else if self.media.borrow().is_some() {
            self.seek_playback(0.)
        } else {
            self.player.previous()
        }
    }
    pub(super) fn playback_rate(&self) -> f64 {
        self.media.borrow().as_ref().map_or(1., |m| m.rate.get())
    }
    pub(super) fn set_playback_rate(&self, rate: f64) -> Result<(), String> {
        self.media.borrow().as_ref().map_or_else(
            || {
                if rate == 1. {
                    Ok::<(), String>(())
                } else {
                    Err("Rate adjustment is available during video and podcast playback".into())
                }
            },
            |m| {
                m.set_rate(rate)?;
                if let Some(picker) = m.rate_picker.upgrade() {
                    picker.set_selected((rate * 4. - 1.).round() as u32);
                }
                Ok(())
            },
        )?;
        self.playhead
            .borrow_mut()
            .rate(rate, glib::monotonic_time());
        Ok(())
    }
    pub(super) fn embedded_track(&self) -> Option<Track> {
        self.media.borrow().as_ref().map(|m| Track {
            id: m.path.to_string_lossy().into_owned(),
            path: m.path.clone(),
            title: m.title.clone(),
            artist: if m.show_feed.is_some() {
                m.stream_artist.clone()
            } else {
                self.db
                    .borrow()
                    .video_tags
                    .get(&m.path)
                    .map(|v| v.song().artist.clone())
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| if m.audio { "Podcast" } else { "Video" }.into())
            },
            album: if m.show_feed.is_some() {
                m.stream_album.clone()
            } else {
                self.db
                    .borrow()
                    .video_tags
                    .get(&m.path)
                    .map(|v| v.song().album.clone())
                    .unwrap_or_default()
            },
            duration: m.snapshot().duration,
            cover: m.artwork.cover.clone(),
            ..Track::default()
        })
    }
}

fn normalized_credit(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn same_credit(a: &str, b: &str) -> bool {
    let a = normalized_credit(a);
    !a.is_empty() && a == normalized_credit(b)
}

fn artist_matches(artist: &str, track: &Track) -> bool {
    same_credit(artist, &track.artist) || same_credit(artist, &track.album_artist)
}

#[derive(Default)]
struct MediaArtwork {
    portrait: Option<PathBuf>,
    cover: Option<PathBuf>,
}

fn media_sidecars(path: &Path) -> MediaArtwork {
    let Some(folder) = path
        .parent()
        .and_then(|parent| std::fs::read_dir(parent).ok())
    else {
        return MediaArtwork::default();
    };
    let mut files: Vec<_> = folder
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| media::is_photo_path(p) && p.is_file())
        .collect();
    files.sort();
    let find = |name: &str| {
        files
            .iter()
            .find(|file| {
                file.file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case(name))
            })
            .cloned()
    };
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    MediaArtwork {
        portrait: find("artist"),
        cover: [stem.as_ref(), "cover", "folder", "front", "album"]
            .into_iter()
            .find_map(find),
    }
}

fn subtitle_sidecar(path: &Path) -> Option<PathBuf> {
    for extension in ["srt", "vtt", "ass", "ssa"] {
        let exact = path.with_extension(extension);
        if exact.is_file() {
            return Some(exact);
        }
    }
    let stem = path.file_stem()?.to_string_lossy();
    let mut matches: Vec<_> = std::fs::read_dir(path.parent()?)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|p| {
            p.is_file()
                && p.extension().is_some_and(|e| {
                    matches!(
                        e.to_string_lossy().to_ascii_lowercase().as_str(),
                        "srt" | "vtt" | "ass" | "ssa"
                    )
                })
                && p.file_stem()
                    .is_some_and(|s| s.to_string_lossy().starts_with(&format!("{stem}.")))
        })
        .collect();
    matches.sort();
    matches.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn video_playlist_advances_in_saved_order_and_stops_at_each_end() {
        let first = VideoPlaylist {
            items: [PathBuf::from("Z.m4v"), PathBuf::from("A.m4v")].into(),
            index: 0,
        };
        assert!(first.step(-1).is_none());
        let second = first.step(1).unwrap();
        assert_eq!(second.items[second.index], Path::new("A.m4v"));
        assert!(second.step(1).is_none());
        assert_eq!(second.step(-1).unwrap().index, 0);
    }

    #[test]
    fn artwork_artist_matching_is_exact_after_case_and_space_normalization() {
        let track = Track {
            artist: "  The   Artist ".into(),
            album_artist: "Various Artists".into(),
            ..Track::default()
        };
        assert!(artist_matches("the artist", &track));
        assert!(artist_matches("VARIOUS ARTISTS", &track));
        assert!(!artist_matches("Artist", &track));
        assert!(!artist_matches("", &Track::default()));
        assert!(!same_credit("", ""));
    }

    #[test]
    fn artist_portrait_and_video_artwork_have_independent_fallbacks() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("clip.mp4");
        assert!(media_sidecars(&video).cover.is_none());
        let cover = dir.path().join("Cover.PNG");
        std::fs::write(&cover, "cover").unwrap();
        assert_eq!(media_sidecars(&video).cover, Some(cover));
        let artist = dir.path().join("artist.jpg");
        std::fs::write(&artist, "portrait").unwrap();
        assert_eq!(media_sidecars(&video).portrait, Some(artist));
        let exact = dir.path().join("clip.webp");
        std::fs::write(&exact, "still").unwrap();
        assert_eq!(media_sidecars(&video).cover, Some(exact));
    }

    #[test]
    fn subtitles_prefer_exact_then_language_tagged_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("clip.mp4");
        let lang = dir.path().join("clip.en.srt");
        std::fs::write(&lang, "subtitles").unwrap();
        assert_eq!(subtitle_sidecar(&video), Some(lang));
        let exact = dir.path().join("clip.srt");
        std::fs::write(&exact, "subtitles").unwrap();
        assert_eq!(subtitle_sidecar(&video), Some(exact));
        assert!(subtitle_sidecar(&dir.path().join("different.mp4")).is_none());
    }
}
