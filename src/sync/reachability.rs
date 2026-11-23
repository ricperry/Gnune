//! Check the same HTTPS URL the phone will use, off the GTK thread.
use super::PairingStatus;
use std::{
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Probe {
    pending: bool,
    result: Option<(Instant, bool)>,
}

pub(super) const PATH: &str = "/api/connection-check";

pub(super) struct RouteCheck {
    origin: String,
    pub(super) identity: String,
    probe: Arc<Mutex<Probe>>,
}

impl RouteCheck {
    pub(super) fn new(origin: &str) -> Self {
        Self {
            origin: origin.to_owned(),
            // Identifies this listener. Independent of pairing codes and grants.
            identity: super::token(),
            probe: Arc::default(),
        }
    }

    pub(super) fn check(&self, force: bool) -> PairingStatus {
        let mut probe = self.probe.lock().unwrap_or_else(|err| err.into_inner());
        if force {
            probe.result = None;
        }
        let due = probe
            .result
            .is_none_or(|(at, ok)| at.elapsed() >= Duration::from_secs(if ok { 60 } else { 10 }));
        if !probe.pending && due {
            probe.pending = true;
            let shared = self.probe.clone();
            let url = format!("{}{PATH}", self.origin);
            let expected = self.identity.clone();
            std::thread::spawn(move || {
                let ok = Command::new("curl")
                    .args([
                        "--disable",
                        "--silent",
                        "--fail",
                        "--noproxy",
                        "*",
                        "--proto",
                        "=https",
                        "--max-filesize",
                        "128",
                        "--connect-timeout",
                        "3",
                        "--max-time",
                        "6",
                        "--url",
                    ])
                    .arg(url)
                    .output()
                    .is_ok_and(|out| out.status.success() && out.stdout == expected.as_bytes());
                let mut probe = shared.lock().unwrap_or_else(|err| err.into_inner());
                probe.result = Some((Instant::now(), ok));
                probe.pending = false;
            });
        }
        match probe.result {
            Some((_, true)) => PairingStatus::Ready,
            Some((_, false)) => PairingStatus::Unavailable,
            None => PairingStatus::Checking,
        }
    }
}
