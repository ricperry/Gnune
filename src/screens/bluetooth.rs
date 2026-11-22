//! Read-only BlueZ presence. Creating this page never scans, pairs, or connects.
use super::*;

#[derive(Debug, PartialEq, Eq)]
struct Phone {
    name: String,
    iphone: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Presence {
    Unavailable,
    Connected(Vec<Phone>),
}

// BlueZ's Icon and Bluetooth major device class identify phones even when their
// owner has renamed them. The name fallback covers devices missing that metadata.
fn phone(mut property: impl FnMut(&str) -> Option<glib::Variant>) -> Option<Phone> {
    if !property("Connected").and_then(|v| v.get::<bool>())? {
        return None;
    }
    let alias = property("Alias").and_then(|v| v.get::<String>());
    let name = property("Name").and_then(|v| v.get::<String>());
    let iphone = [alias.as_deref(), name.as_deref()]
        .into_iter()
        .flatten()
        .any(|name| name.to_ascii_lowercase().contains("iphone"));
    let icon = property("Icon").and_then(|v| v.get::<String>());
    let class = property("Class").and_then(|v| v.get::<u32>());
    let phone_icon = icon
        .as_deref()
        .is_some_and(|icon| icon == "phone" || icon.starts_with("phone-"));
    let unknown_icon = icon.as_deref().is_none_or(str::is_empty);
    let phone_class = class.is_some_and(|class| class & 0x1f00 == 0x0200);
    let unknown_class = class.is_none_or(|class| class & 0x1f00 == 0);
    if !phone_icon && !phone_class && !(iphone && unknown_class && unknown_icon) {
        return None;
    }
    let name = alias
        .filter(|name| !name.trim().is_empty())
        .or_else(|| name.filter(|name| !name.trim().is_empty()))
        .unwrap_or_else(|| "Connected phone".into());
    Some(Phone { name, iphone })
}

fn presence(manager: &gio::DBusObjectManagerClient) -> Presence {
    if manager.name_owner().is_none() || manager.connection().is_closed() {
        return Presence::Unavailable;
    }
    let mut phones: Vec<_> = manager
        .objects()
        .iter()
        .filter_map(|object| {
            let proxy = gio::prelude::DBusObjectExt::interface(object, "org.bluez.Device1")?
                .downcast::<gio::DBusProxy>()
                .ok()?;
            phone(|name| proxy.cached_property(name))
        })
        .collect();
    phones.sort_by_key(|phone| (phone.name.to_lowercase(), phone.name.clone()));
    Presence::Connected(phones)
}

fn show(list: &gtk::Box, presence: &Presence) {
    clear(list);
    match presence {
        Presence::Unavailable => list.append(&label(
            "Bluetooth status unavailable. Check that Bluetooth is enabled and Gnune can access it.",
            "subtitle",
        )),
        Presence::Connected(phones) if phones.is_empty() => list.append(&label(
            "No connected phones. Connect your phone in GNOME Settings → Bluetooth.",
            "subtitle",
        )),
        Presence::Connected(phones) => {
            for phone in phones {
                let device = row(12);
                device.append(&gtk::Image::from_icon_name("phone-symbolic"));
                let details = col(2);
                details.append(&label(&phone.name, "track"));
                details.append(&label(
                    if phone.iphone {
                        "iPhone · Connected"
                    } else {
                        "Phone · Connected"
                    },
                    "subtitle",
                ));
                device.append(&details);
                list.append(&device);
            }
        }
    }
}

pub(super) fn panel() -> gtk::Box {
    let panel = col(12);
    panel.append(&label("Connected by Bluetooth", "section"));
    let list = col(12);
    list.append(&label("Checking connected phones…", "subtitle"));
    panel.append(&list);
    let weak_panel = panel.downgrade();
    let weak_list = list.downgrade();
    glib::spawn_future_local(async move {
        // Failed bus access can recover without closing the page. A manager with
        // no BlueZ owner remains subscribed and repopulates after daemon restart.
        let manager = loop {
            match gio::DBusObjectManagerClient::new_for_bus_future(
                gio::BusType::System,
                gio::DBusObjectManagerClientFlags::DO_NOT_AUTO_START,
                "org.bluez",
                "/",
            )
            .await
            {
                Ok(manager) => break manager,
                Err(_) => {
                    let Some(list) = weak_list.upgrade() else {
                        return;
                    };
                    show(&list, &Presence::Unavailable);
                    drop(list);
                    glib::timeout_future_seconds(5).await;
                    if weak_panel.upgrade().is_none() {
                        return;
                    }
                }
            }
        };
        let Some(panel) = weak_panel.upgrade() else {
            return;
        };
        if panel.root().is_none() {
            return;
        }
        let previous = Rc::new(RefCell::new(None));
        let weak_manager = manager.downgrade();
        let refresh = Rc::new(move || {
            let (Some(list), Some(manager)) = (weak_list.upgrade(), weak_manager.upgrade()) else {
                return;
            };
            let current = presence(&manager);
            if previous.borrow().as_ref() != Some(&current) {
                show(&list, &current);
                previous.replace(Some(current));
            }
        });
        refresh();
        let changed = refresh.clone();
        manager.connect_object_added(move |_, _| changed());
        let changed = refresh.clone();
        manager.connect_object_removed(move |_, _| changed());
        let changed = refresh.clone();
        manager.connect_interface_added(move |_, _, _| changed());
        let changed = refresh.clone();
        manager.connect_interface_removed(move |_, _, _| changed());
        let changed = refresh.clone();
        manager.connect_notify_local(Some("name-owner"), move |_, _| changed());
        // The typed signal requires Send + Sync; this manager and GTK widgets
        // instead stay on the main context that created the proxy.
        manager.connect_local("interface-proxy-properties-changed", false, move |_| {
            refresh();
            None
        });
        // Keep the subscription only for the rendered page's lifetime.
        let monitor = RefCell::new(Some(manager));
        panel.connect_unrealize(move |_| {
            monitor.take();
        });
    });
    panel
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detected(properties: &[(&str, glib::Variant)]) -> Option<Phone> {
        phone(|name| {
            properties
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.clone())
        })
    }

    #[test]
    fn connected_phones_use_bluez_metadata_and_alias() {
        let connected = ("Connected", true.to_variant());
        assert_eq!(
            detected(&[
                connected.clone(),
                ("Alias", "My phone".to_variant()),
                ("Name", "iPhone".to_variant()),
                ("Icon", "phone".to_variant()),
            ]),
            Some(Phone {
                name: "My phone".into(),
                iphone: true,
            })
        );
        assert_eq!(
            detected(&[
                connected.clone(),
                ("Name", "Pixel".to_variant()),
                ("Class", 0x5a020cu32.to_variant()),
            ]),
            Some(Phone {
                name: "Pixel".into(),
                iphone: false,
            })
        );
        assert!(detected(&[connected, ("Name", "iPhone".to_variant())]).is_some());
        assert!(
            detected(&[
                ("Connected", true.to_variant()),
                ("Alias", "Renamed phone".to_variant()),
                ("Icon", "phone".to_variant()),
            ])
            .is_some()
        );
    }

    #[test]
    fn ignores_disconnected_phones_and_other_connected_devices() {
        assert!(
            detected(&[
                ("Connected", false.to_variant()),
                ("Icon", "phone".to_variant()),
            ])
            .is_none()
        );
        assert!(
            detected(&[
                ("Connected", true.to_variant()),
                ("Icon", "audio-headset".to_variant()),
                ("Name", "iPhone headset".to_variant()),
            ])
            .is_none()
        );
        assert!(detected(&[("Icon", "phone".to_variant())]).is_none());
        assert!(
            detected(&[
                ("Connected", true.to_variant()),
                ("Icon", "audio-headset".to_variant()),
                ("Name", "iPhone headphones".to_variant()),
                ("Class", 0x240404u32.to_variant()),
            ])
            .is_none()
        );
    }
}
