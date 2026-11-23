//! GNOME media controls through the standard MPRIS session-bus interfaces.
//! Call `mpris::register(&ui)` once after constructing the window.
use super::*;
use glib::variant::{ObjectPath, ToVariant};
use gtk::prelude::*;
use gtk::{gio, glib};
use std::{
    cell::Cell,
    collections::BTreeMap,
    time::{Duration, Instant},
};

const PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT: &str = "org.mpris.MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const XML: &str = r#"<node>
<interface name="org.mpris.MediaPlayer2">
 <method name="Raise"/><method name="Quit"/>
 <property name="CanQuit" type="b" access="read"/>
 <property name="CanRaise" type="b" access="read"/>
 <property name="HasTrackList" type="b" access="read"/>
 <property name="Identity" type="s" access="read"/>
 <property name="SupportedUriSchemes" type="as" access="read"/>
 <property name="SupportedMimeTypes" type="as" access="read"/>
</interface>
<interface name="org.mpris.MediaPlayer2.Player">
 <method name="Next"/><method name="Previous"/><method name="Pause"/>
 <method name="PlayPause"/><method name="Stop"/><method name="Play"/>
 <method name="Seek"><arg name="Offset" type="x" direction="in"/></method>
 <method name="SetPosition"><arg name="TrackId" type="o" direction="in"/><arg name="Position" type="x" direction="in"/></method>
 <method name="OpenUri"><arg name="Uri" type="s" direction="in"/></method>
 <signal name="Seeked"><arg name="Position" type="x"/></signal>
 <property name="PlaybackStatus" type="s" access="read"/>
 <property name="Rate" type="d" access="readwrite"/>
 <property name="Metadata" type="a{sv}" access="read"/>
 <property name="Volume" type="d" access="readwrite"/>
 <property name="Position" type="x" access="read"/>
 <property name="MinimumRate" type="d" access="read"/>
 <property name="MaximumRate" type="d" access="read"/>
 <property name="CanGoNext" type="b" access="read"/>
 <property name="CanGoPrevious" type="b" access="read"/>
 <property name="CanPlay" type="b" access="read"/>
 <property name="CanPause" type="b" access="read"/>
 <property name="CanSeek" type="b" access="read"/>
 <property name="CanControl" type="b" access="read"/>
</interface>
</node>"#;

fn track_path(track: &Track) -> ObjectPath {
    // IDs from the importer are SHA-256 hex; retain a valid object path for
    // caller-provided tracks as well.
    let id: String = track
        .id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    ObjectPath::try_from(format!("/org/mpris/MediaPlayer2/track/t{id}"))
        .expect("sanitized object path")
}

fn micros(seconds: f64) -> i64 {
    (seconds.max(0.0) * 1_000_000.0) as i64
}

fn metadata(u: &Ui) -> BTreeMap<String, glib::Variant> {
    let mut values = BTreeMap::new();
    if let Some(track) = u.current() {
        values.insert("mpris:trackid".into(), track_path(&track).to_variant());
        values.insert("mpris:length".into(), micros(track.duration).to_variant());
        values.insert("xesam:title".into(), track.title.to_variant());
        values.insert(
            "xesam:artist".into(),
            vec![track.artist.clone()].to_variant(),
        );
        values.insert("xesam:album".into(), track.album.to_variant());
        values.insert(
            "xesam:albumArtist".into(),
            vec![track.album_artist.clone()].to_variant(),
        );
        values.insert("xesam:genre".into(), vec![track.genre.clone()].to_variant());
        if let Some(number) = track.track_number {
            values.insert("xesam:trackNumber".into(), (number as i32).to_variant());
        }
        if let Some(number) = track.disc_number {
            values.insert("xesam:discNumber".into(), (number as i32).to_variant());
        }
        if let Some(uri) = u
            .media
            .borrow()
            .as_ref()
            .and_then(|media| media.stream_uri.clone())
        {
            values.insert("xesam:url".into(), uri.to_variant());
        } else if let Ok(url) = url::Url::from_file_path(&track.path) {
            values.insert("xesam:url".into(), url.as_str().to_variant());
        }
        if let Some(path) = u.playback_artwork()
            && let Ok(url) = url::Url::from_file_path(path)
        {
            values.insert("mpris:artUrl".into(), url.as_str().to_variant());
        }
    }
    values
}

fn properties(u: &Ui, stopped: bool) -> BTreeMap<String, glib::Variant> {
    let state = u.playback_state();
    let current = u.media.borrow().is_some() || u.current.borrow().is_some();
    let status = if stopped || state.finished || !current {
        "Stopped"
    } else if state.paused {
        "Paused"
    } else {
        "Playing"
    };
    BTreeMap::from([
        ("PlaybackStatus".into(), status.to_variant()),
        ("Rate".into(), u.playback_rate().to_variant()),
        (
            "MinimumRate".into(),
            (if u.media.borrow().is_some() {
                0.25f64
            } else {
                1.
            })
            .to_variant(),
        ),
        (
            "MaximumRate".into(),
            (if u.media.borrow().is_some() {
                4.0f64
            } else {
                1.
            })
            .to_variant(),
        ),
        ("Metadata".into(), metadata(u).to_variant()),
        ("Volume".into(), (state.volume / 100.0).to_variant()),
        (
            "CanGoNext".into(),
            (u.has_next_video()
                || (state
                    .queue_index
                    .is_some_and(|i| i + 1 < u.queue.borrow().len())
                    && !state.finished))
                .to_variant(),
        ),
        (
            "CanGoPrevious".into(),
            (u.media.borrow().is_some() || state.queue_index.is_some_and(|i| i > 0)).to_variant(),
        ),
        (
            "CanPlay".into(),
            (u.media.borrow().is_some() || !u.db.borrow().tracks.is_empty()).to_variant(),
        ),
        ("CanPause".into(), current.to_variant()),
        (
            "CanSeek".into(),
            (current && state.duration > 0.0).to_variant(),
        ),
        ("CanControl".into(), true.to_variant()),
    ])
}

pub fn register(u: &Rc<Ui>) {
    let connection = match gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!("MPRIS session bus unavailable: {error}");
            return;
        }
    };
    let node = gio::DBusNodeInfo::for_xml(XML).expect("valid MPRIS introspection");
    let stopped = Rc::new(Cell::new(false));
    let mut registrations = Vec::new();
    for interface in [ROOT, PLAYER] {
        let weak_method = Rc::downgrade(u);
        let weak_get = weak_method.clone();
        let weak_set = weak_method.clone();
        let stopped_method = stopped.clone();
        let stopped_get = stopped.clone();
        let info = node.lookup_interface(interface).expect("MPRIS interface");
        let registration = connection
            .register_object(PATH, &info)
            .method_call(move |_, _, _, _, method, parameters, invocation| {
                let Some(u) = weak_method.upgrade() else {
                    invocation.return_dbus_error(
                        "org.freedesktop.DBus.Error.Failed",
                        "Player has closed",
                    );
                    return;
                };
                let state = u.playback_state();
                let result = match method {
                    "Raise" => {
                        u.win.present();
                        Ok(())
                    }
                    "Quit" => {
                        u.win.close();
                        Ok(())
                    }
                    "PlayPause" => {
                        stopped_method.set(false);
                        u.toggle();
                        Ok(())
                    }
                    "Play" => {
                        stopped_method.set(false);
                        if state.paused || state.finished || u.current().is_none() {
                            u.toggle();
                        }
                        Ok(())
                    }
                    "Pause" => u.set_playback_paused(true),
                    "Stop" => {
                        stopped_method.set(true);
                        u.stop_playback()
                    }
                    "Next" => u.next_playback(),
                    "Previous" => u.previous_playback(),
                    "Seek" => {
                        if let Some((offset,)) = parameters.get::<(i64,)>() {
                            let position = (state.position + offset as f64 / 1_000_000.0).max(0.0);
                            if state.duration > 0.0 && position > state.duration {
                                u.next_playback()
                            } else {
                                u.seek_playback(position)
                            }
                        } else {
                            Err("Invalid seek offset".into())
                        }
                    }
                    "SetPosition" => {
                        if let Some((id, position)) = parameters.get::<(ObjectPath, i64)>() {
                            if position >= 0
                                && position as f64 / 1_000_000.0 <= state.duration
                                && u.current().is_some_and(|t| track_path(&t) == id)
                            {
                                u.seek_playback(position as f64 / 1_000_000.0)
                            } else {
                                Ok(())
                            }
                        } else {
                            Err("Invalid position".into())
                        }
                    }
                    "OpenUri" => {
                        invocation.return_dbus_error(
                            "org.freedesktop.DBus.Error.NotSupported",
                            "Import local music through Gnune",
                        );
                        return;
                    }
                    _ => {
                        invocation.return_dbus_error(
                            "org.freedesktop.DBus.Error.UnknownMethod",
                            "Unknown media method",
                        );
                        return;
                    }
                };
                match result {
                    Ok(()) => invocation.return_value(None),
                    Err(error) => {
                        u.command(Err(error.clone()));
                        invocation.return_dbus_error("org.freedesktop.DBus.Error.Failed", &error);
                    }
                }
            })
            .property(move |_, _, _, interface, property| {
                if interface == ROOT {
                    return match property {
                        "Identity" => "Gnune".to_variant(),
                        "CanQuit" | "CanRaise" => true.to_variant(),
                        "HasTrackList" => false.to_variant(),
                        _ => Vec::<String>::new().to_variant(),
                    };
                }
                if let Some(u) = weak_get.upgrade() {
                    if property == "Position" {
                        return micros(u.playback_state().position).to_variant();
                    }
                    if let Some(value) = properties(&u, stopped_get.get()).remove(property) {
                        return value;
                    }
                }
                false.to_variant()
            })
            .set_property(move |_, _, _, _, property, value| {
                let Some(u) = weak_set.upgrade() else {
                    return false;
                };
                match property {
                    "Volume" => {
                        let Some(volume) = value.get::<f64>().filter(|v| v.is_finite()) else {
                            return false;
                        };
                        let result = u.set_playback_volume(volume.max(0.0) * 100.0);
                        let success = result.is_ok();
                        u.command(result);
                        success
                    }
                    "Rate" => value
                        .get::<f64>()
                        .is_some_and(|rate| u.set_playback_rate(rate).is_ok()),
                    _ => false,
                }
            })
            .build();
        match registration {
            Ok(id) => registrations.push(id),
            Err(error) => {
                for id in registrations {
                    let _ = connection.unregister_object(id);
                }
                eprintln!("MPRIS registration failed: {error}");
                return;
            }
        }
    }
    let owner = gio::bus_own_name_on_connection(
        &connection,
        "org.mpris.MediaPlayer2.Gnune",
        gio::BusNameOwnerFlags::NONE,
        |_, _| {},
        |_, _| {},
    );
    let mut owner = Some(owner);
    let weak = Rc::downgrade(u);
    let mut previous = properties(u, false);
    let mut previous_state = u.playback_state();
    let mut previous_time = Instant::now();
    glib::timeout_add_local(Duration::from_millis(200), move || {
        let Some(u) = weak.upgrade() else {
            for id in registrations.drain(..) {
                let _ = connection.unregister_object(id);
            }
            if let Some(owner) = owner.take() {
                gio::bus_unown_name(owner);
            }
            return glib::ControlFlow::Break;
        };
        let state = u.playback_state();
        if !state.paused
            && (previous_state.paused
                || previous_state.finished
                || state.track_id != previous_state.track_id)
        {
            stopped.set(false);
        }
        let current = properties(&u, stopped.get());
        let changed: BTreeMap<_, _> = current
            .iter()
            .filter(|(key, value)| previous.get(*key) != Some(*value))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        if !changed.is_empty() {
            let _ = connection.emit_signal(
                None,
                PATH,
                "org.freedesktop.DBus.Properties",
                "PropertiesChanged",
                Some(&(PLAYER, changed, Vec::<String>::new()).to_variant()),
            );
        }
        let now = Instant::now();
        let expected = previous_state.position
            + if previous_state.paused || previous_state.finished {
                0.0
            } else {
                now.duration_since(previous_time).as_secs_f64() * u.playback_rate()
            };
        if state.track_id.is_some()
            && state.track_id == previous_state.track_id
            && (state.position - expected).abs() > 0.25
        {
            let _ = connection.emit_signal(
                None,
                PATH,
                PLAYER,
                "Seeked",
                Some(&(micros(state.position),).to_variant()),
            );
        }
        previous = current;
        previous_state = state;
        previous_time = now;
        glib::ControlFlow::Continue
    });
}
