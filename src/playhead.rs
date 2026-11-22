//! Display-only playback projection. Backend samples remain the source of truth.
use gnune::player::PlaybackState;

// A missing/stalled backend must never leave a permanently advancing display.
const MAX_LEAD_US: i64 = 500_000;

#[derive(Default)]
pub struct Playhead {
    track: Option<String>,
    position: f64,
    sampled_at: i64,
    duration: f64,
    rate: f64,
    moving: bool,
    seek_after: Option<i64>,
    pause_pending: Option<(bool, i64)>,
}

impl Playhead {
    pub fn sample(&mut self, state: &PlaybackState, rate: f64, sampled_at: i64, now: i64) {
        let changed = self.track != state.track_id;
        if changed {
            self.seek_after = None;
            self.pause_pending = None;
        }
        self.track.clone_from(&state.track_id);
        self.duration = state.duration.max(0.);
        self.rate = rate;
        if let Some((paused, until)) = self.pause_pending
            && (state.paused == paused || now >= until)
        {
            self.pause_pending = None;
        }
        let paused = self.pause_pending.map_or(state.paused, |(p, _)| p);
        self.moving = state.loaded && !paused && !state.finished && self.track.is_some();
        // A successful seek command can precede the backend's observed position.
        if self.seek_after.is_some_and(|after| sampled_at <= after) {
            self.moving = false;
            return;
        }
        self.seek_after = None;
        // Repeated GStreamer queries at a stalled position are not new progress.
        if changed || state.position != self.position || !self.moving {
            self.position = state.position.max(0.);
            self.sampled_at = sampled_at.min(now);
        }
    }

    pub fn position(&self, now: i64) -> f64 {
        let elapsed = if self.moving {
            (now - self.sampled_at).clamp(0, MAX_LEAD_US) as f64 / 1_000_000.
        } else {
            0.
        };
        let value = self.position + elapsed * self.rate;
        if self.duration > 0. {
            value.min(self.duration)
        } else {
            value
        }
    }

    pub fn animating(&self, now: i64) -> bool {
        self.moving && now - self.sampled_at < MAX_LEAD_US && self.position(now) < self.duration
    }

    pub fn seek(&mut self, position: f64, now: i64) {
        self.position = position.max(0.);
        self.sampled_at = now;
        self.seek_after = Some(now);
        self.moving = false;
    }

    pub fn pause(&mut self, paused: bool, now: i64) {
        self.position = self.position(now);
        self.sampled_at = now;
        self.pause_pending = Some((paused, now + MAX_LEAD_US));
        self.moving = !paused;
    }

    pub fn rate(&mut self, rate: f64, now: i64) {
        self.position = self.position(now);
        self.sampled_at = now;
        self.rate = rate;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing() -> PlaybackState {
        PlaybackState {
            track_id: Some("song".into()),
            loaded: true,
            paused: false,
            position: 10.,
            duration: 100.,
            ..Default::default()
        }
    }

    #[test]
    fn projects_at_frame_times_without_changing_authoritative_state() {
        let state = playing();
        let mut p = Playhead::default();
        p.sample(&state, 1., 1_000_000, 1_000_000);
        assert!((p.position(1_016_667) - 10.016667).abs() < 1e-8);
        assert_eq!(p.position(1_160_000), 10.16);
        assert_eq!(state.position, 10.);
        assert!(p.animating(1_160_000));
    }

    #[test]
    fn duplicate_samples_and_stalls_stop_projection() {
        let mut p = Playhead::default();
        p.sample(&playing(), 1., 0, 0);
        p.sample(&playing(), 1., 320_000, 320_000);
        assert_eq!(p.position(900_000), 10.5);
        assert!(!p.animating(900_000));
        let mut state = playing();
        state.position = 10.7;
        p.sample(&state, 1., 900_000, 900_000);
        assert!(p.animating(900_000));
    }

    #[test]
    fn seeks_wait_for_fresh_samples_and_tracks_reset_immediately() {
        let mut p = Playhead::default();
        p.sample(&playing(), 1., 0, 0);
        p.seek(50., 100_000);
        p.sample(&playing(), 1., 0, 150_000);
        assert_eq!(p.position(150_000), 50.);
        assert!(!p.animating(150_000));
        let mut state = playing();
        state.position = 50.1;
        p.sample(&state, 1., 200_000, 200_000);
        assert_eq!(p.position(210_000), 50.11);
        state.track_id = Some("next".into());
        state.position = 0.;
        p.sample(&state, 1., 220_000, 220_000);
        assert_eq!(p.position(220_000), 0.);
    }

    #[test]
    fn pause_rate_discontinuities_finish_and_duration() {
        let mut p = Playhead::default();
        let mut state = playing();
        p.sample(&state, 1., 0, 0);
        p.rate(2., 100_000);
        assert!((p.position(200_000) - 10.3).abs() < 1e-8);
        p.pause(true, 200_000);
        assert!((p.position(300_000) - 10.3).abs() < 1e-8);
        assert!(!p.animating(300_000));
        state.paused = true;
        state.position = 10.25;
        p.sample(&state, 2., 300_000, 300_000);
        assert_eq!(p.position(400_000), 10.25);
        state.paused = false;
        state.position = 99.9;
        p.sample(&state, 2., 500_000, 500_000);
        assert_eq!(p.position(600_000), 100.);
        assert!(!p.animating(600_000));
        state.finished = true;
        p.sample(&state, 2., 700_000, 700_000);
        assert!(!p.animating(700_000));
    }
}
