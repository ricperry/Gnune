//! Durable, explicit desktop sharing consent. Never execute the setup env file.
use super::{Grant, ShareMode};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Connection {
    pub bind: SocketAddr,
    pub origin: String,
    #[serde(default, skip_serializing_if = "empty_path")]
    pub cert: PathBuf,
    #[serde(default, skip_serializing_if = "empty_path")]
    pub key: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serve_socket: Option<PathBuf>,
}
fn empty_path(value: &Path) -> bool {
    value.as_os_str().is_empty()
}
impl Connection {
    /// The host helper gives Serve a private Unix socket and a canonical HTTPS origin.
    pub(super) fn is_tailscale(&self) -> bool {
        self.bind.ip().is_loopback()
            && self
                .serve_socket
                .as_ref()
                .is_some_and(|path| path.is_absolute())
            && url::Url::parse(&self.origin).ok().is_some_and(|url| {
                url.scheme() == "https"
                    && url.port_or_known_default() == Some(self.bind.port())
                    && url.origin().ascii_serialization() == self.origin
                    && url.host_str().is_some_and(|host| host.ends_with(".ts.net"))
            })
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Saved {
    pub enabled: bool,
    pub connection: Connection,
    pub mode: ShareMode,
    pub include_video: bool,
    #[serde(default)]
    pub include_podcasts: bool,
    // Queue consent is an exact file-identity snapshot, independent of playback.
    pub queue: Option<Grant>,
    pub skipped: usize,
}
fn root() -> PathBuf {
    gtk::glib::user_config_dir().join("gnune")
}
fn path() -> PathBuf {
    root().join("sharing.json")
}
pub(super) fn load() -> Result<Option<Saved>, String> {
    load_at(&path())
}
fn load_at(path: &Path) -> Result<Option<Saved>, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("Cannot read saved sharing settings: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Cannot read saved sharing settings: {e}")),
    }
}
pub(super) fn save(saved: &Saved) -> Result<(), String> {
    save_at(&path(), saved)
}
fn save_at(path: &Path, saved: &Saved) -> Result<(), String> {
    use std::io::Write;
    let parent = path.parent().ok_or("Invalid sharing settings path")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut file, saved).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.error.to_string())?;
    fs::File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| e.to_string())
}
pub(super) fn disable() -> Result<(), String> {
    let _guard = migration_guard()?;
    if let Some(mut saved) = load()? {
        saved.enabled = false;
        save(&saved)?;
    }
    Ok(())
}
const NAMES: [&str; 4] = [
    "GNUNE_SYNC_BIND",
    "GNUNE_SYNC_ORIGIN",
    "GNUNE_SYNC_CERT",
    "GNUNE_SYNC_KEY",
];
fn literal_env(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut values = BTreeMap::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let line = line.strip_prefix("export ").unwrap_or(line).trim();
        let Some((name, raw)) = line.split_once('=') else {
            return Err("Sharing env.sh must contain literal GNUNE_SYNC_* assignments.".into());
        };
        let name = name.trim();
        if !NAMES.contains(&name) {
            return Err("Sharing env.sh must contain only GNUNE_SYNC_* assignments.".into());
        }
        let raw = raw.trim();
        let value = if (raw.starts_with('\'') && raw.ends_with('\''))
            || (raw.starts_with('"') && raw.ends_with('"'))
        {
            raw.get(1..raw.len().saturating_sub(1))
                .ok_or("Invalid quoted sharing setting")?
        } else {
            raw
        };
        if value.is_empty()
            || value.contains([
                '$', '`', '\\', '\n', '\r', '\'', '"', ';', '|', '&', '<', '>',
            ])
            || (!raw.starts_with(['\'', '"']) && value.chars().any(char::is_whitespace))
        {
            return Err("Sharing env.sh supports literal values only; use sharing.json or environment variables for other values.".into());
        }
        values.insert(name.into(), value.into());
    }
    Ok(values)
}
#[derive(Deserialize)]
struct SetupReceipt {
    state: String,
    before: Option<Connection>,
    after: Connection,
    host: String,
    port: u16,
    target: String,
}

// An applied host migration takes precedence over old GNUNE_SYNC_* exports.
// Older builds could drop the socket field or rewrite the pre-migration endpoint.
fn managed_connection(
    dir: &Path,
    saved: Option<&Connection>,
) -> Result<Option<Connection>, String> {
    let bytes = match fs::read(dir.join("tailscale-setup.json")) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(saved.filter(|c| c.is_tailscale()).cloned());
        }
        Err(e) => return Err(format!("Cannot read Tailscale setup: {e}")),
    };
    let receipt: SetupReceipt =
        serde_json::from_slice(&bytes).map_err(|e| format!("Cannot read Tailscale setup: {e}"))?;
    let after = receipt.after;
    let socket = dir.join("tailscale-private/api.sock");
    if receipt.state != "applied"
        || !after.is_tailscale()
        || after.bind.port() != receipt.port
        || url::Url::parse(&after.origin)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .as_deref()
            != Some(&receipt.host)
        || after.serve_socket.as_ref() != Some(&socket)
        || receipt.target != format!("unix:{}", socket.display())
    {
        return Err("Tailscale setup is incomplete. Review the host setup before pairing.".into());
    }
    if let Some(saved) = saved {
        let same_endpoint = |c: &Connection| saved.bind == c.bind && saved.origin == c.origin;
        if !same_endpoint(&after) && !receipt.before.as_ref().is_some_and(same_endpoint) {
            return Err("The saved endpoint differs from Tailscale setup. Review the host setup before pairing.".into());
        }
    }
    Ok(Some(after))
}
pub(super) fn connection(saved: Option<&Saved>) -> Result<Connection, String> {
    if let Some(connection) = managed_connection(&root(), saved.map(|s| &s.connection))? {
        return Ok(connection);
    }
    let fallback_path = root().join("phone-sharing/env.sh");
    let mut values = BTreeMap::new();
    for name in NAMES {
        if let Some(value) = std::env::var_os(name) {
            values.insert(
                name.to_owned(),
                value
                    .into_string()
                    .map_err(|_| format!("{name} must be UTF-8"))?,
            );
        }
    }
    if let Some(saved) = saved {
        for (name, value) in [
            (NAMES[0], saved.connection.bind.to_string()),
            (NAMES[1], saved.connection.origin.clone()),
            (
                NAMES[2],
                saved.connection.cert.to_string_lossy().into_owned(),
            ),
            (
                NAMES[3],
                saved.connection.key.to_string_lossy().into_owned(),
            ),
        ] {
            values.entry(name.into()).or_insert(value);
        }
    }
    if NAMES.iter().any(|name| !values.contains_key(*name)) {
        match fs::read_to_string(fallback_path) {
            Ok(text) => {
                for (name, value) in literal_env(&text)? {
                    values.entry(name).or_insert(value);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("Cannot read sharing env.sh: {e}")),
        }
    }
    let mut connection = parse_connection(&values)?;
    if let Some(saved) = saved
        && saved.connection.bind == connection.bind
        && saved.connection.origin == connection.origin
    {
        connection.serve_socket = saved.connection.serve_socket.clone();
    }
    Ok(connection)
}
fn parse_connection(values: &BTreeMap<String, String>) -> Result<Connection, String> {
    let get = |name: &str| {
        values
            .get(name)
            .filter(|v| !v.trim().is_empty())
            .cloned()
            .ok_or_else(|| format!("Set {name} before enabling phone sharing"))
    };
    let bind: SocketAddr = get(NAMES[0])?
        .parse()
        .map_err(|_| "GNUNE_SYNC_BIND must be an IP address and port")?;
    if bind.ip().is_unspecified() {
        return Err(
            "GNUNE_SYNC_BIND must name a specific local interface, not 0.0.0.0 or ::".into(),
        );
    }
    let origin = url::Url::parse(&get(NAMES[1])?).map_err(|e| e.to_string())?;
    if origin.scheme() != "https"
        || origin.host_str().is_none()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.port_or_known_default() != Some(bind.port())
    {
        return Err(
            "GNUNE_SYNC_ORIGIN must be an HTTPS origin with the same port as GNUNE_SYNC_BIND"
                .into(),
        );
    }
    let adopt_path = |name: &str| -> Result<PathBuf, String> {
        let path = PathBuf::from(get(name)?);
        if path.is_absolute() {
            Ok(path)
        } else {
            // Keep symlink paths intact so certificate rotation still works.
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .map_err(|e| format!("Cannot resolve relative {name} path: {e}"))
        }
    };
    Ok(Connection {
        bind,
        origin: origin.origin().ascii_serialization(),
        cert: adopt_path(NAMES[2])?,
        key: adopt_path(NAMES[3])?,
        serve_socket: None,
    })
}
pub(super) fn tls(connection: &Connection) -> Result<(Vec<u8>, Vec<u8>), String> {
    let cert =
        fs::read(&connection.cert).map_err(|e| format!("Cannot read sharing certificate: {e}"))?;
    let key =
        fs::read(&connection.key).map_err(|e| format!("Cannot read sharing private key: {e}"))?;
    validate_tls(&cert, &key)?;
    Ok((cert, key))
}
fn validate_tls(cert: &[u8], key: &[u8]) -> Result<(), String> {
    let chain = openssl::x509::X509::stack_from_pem(cert)
        .map_err(|e| format!("Invalid sharing certificate PEM: {e}"))?;
    let leaf = chain.first().ok_or("Sharing certificate chain is empty")?;
    let private = openssl::pkey::PKey::private_key_from_pem_callback(key, |_| Ok(0))
        .map_err(|e| format!("Invalid or encrypted sharing private key: {e}"))?;
    if !leaf
        .public_key()
        .map_err(|e| e.to_string())?
        .public_eq(&private)
    {
        return Err("Sharing certificate and private key do not match".into());
    }
    Ok(())
}

/// A helper holds an exclusive flock while migrating endpoints. Session startup
/// takes the shared lock before reading or writing remembered sharing state.
pub(super) fn migration_guard() -> Result<fs::File, String> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::create_dir_all(root()).map_err(|e| e.to_string())?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path().with_extension("lock"))
        .map_err(|e| e.to_string())?;
    file.try_lock_shared().map_err(|_| {
        "Phone sharing setup is running. Close Gnune and finish setup, then retry sharing."
            .to_string()
    })?;
    Ok(file)
}
pub(super) fn unix_server(path: &Path) -> Result<tiny_http::Server, String> {
    use std::os::unix::{
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
        net::UnixStream,
    };
    let parent = path
        .parent()
        .filter(|_| path.is_absolute())
        .ok_or("Serve socket must be an absolute path")?;
    let uid = fs::metadata("/proc/self").map_err(|e| e.to_string())?.uid();
    let metadata = fs::symlink_metadata(parent)
        .map_err(|_| "Private Serve directory is missing; run the host setup helper")?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err("Serve socket directory must be owned by this user with mode 0700".into());
    }
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_socket() || metadata.uid() != uid {
            return Err("Serve socket path is occupied by another file".into());
        }
        match UnixStream::connect(path) {
            Ok(_) => return Err("Another Gnune listener is using the private Serve socket".into()),
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                fs::remove_file(path).map_err(|e| e.to_string())?
            }
            Err(error) => return Err(format!("Cannot inspect existing Serve socket: {error}")),
        }
    }
    let server = tiny_http::Server::http_unix(path).map_err(|e| e.to_string())?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    Ok(server)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn applied_tailscale_setup_recovers_a_stale_endpoint_and_supports_first_pairing() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("tailscale-private/api.sock");
        let origin = "https://desktop.example.ts.net:8443";
        let before = serde_json::json!({"bind":"192.0.2.1:8443", "origin":"https://192.0.2.1:8443", "cert":"/old.pem", "key":"/old.key"});
        let after =
            serde_json::json!({"bind":"127.0.0.1:8443", "origin":origin, "serve_socket":socket});
        let mut receipt = serde_json::json!({"state":"applied", "host":"desktop.example.ts.net",
            "port":8443, "target":format!("unix:{}", socket.display()), "before":before, "after":after});
        let path = dir.path().join("tailscale-setup.json");
        fs::write(&path, receipt.to_string()).unwrap();
        let old = serde_json::from_value(before).unwrap();
        let endpoint = managed_connection(dir.path(), Some(&old)).unwrap().unwrap();
        assert!(endpoint.is_tailscale());
        assert_eq!(endpoint.origin, origin);
        receipt["before"] = serde_json::Value::Null;
        fs::write(&path, receipt.to_string()).unwrap();
        let fresh = managed_connection(dir.path(), None).unwrap().unwrap();
        assert_eq!(serde_json::to_value(fresh).unwrap(), after);
        assert!(managed_connection(dir.path(), Some(&old)).is_err());
        receipt["state"] = "preparing".into();
        fs::write(path, receipt.to_string()).unwrap();
        assert!(managed_connection(dir.path(), None).is_err());
    }
    #[test]
    fn setup_file_is_literal_and_never_evaluated() {
        let values = literal_env("export GNUNE_SYNC_BIND=192.0.2.1:8443\nGNUNE_SYNC_ORIGIN='https://gnune.local:8443'\nGNUNE_SYNC_CERT=\"/tmp/a b.pem\"\nGNUNE_SYNC_KEY=/tmp/key.pem\n").unwrap();
        assert_eq!(
            parse_connection(&values).unwrap().cert,
            PathBuf::from("/tmp/a b.pem")
        );
        assert!(literal_env("GNUNE_SYNC_KEY=$(touch /tmp/no)").is_err());
        assert!(literal_env("GNUNE_SYNC_KEY=/tmp/a;touch /tmp/no").is_err());
        assert!(literal_env("GNUNE_SYNC_KEY=`id`").is_err());
        assert!(literal_env("PATH=/bad").is_err());
    }
    #[test]
    fn relative_tls_paths_are_adopted_without_resolving_symlink_targets() {
        let values = literal_env("GNUNE_SYNC_BIND=192.0.2.1:8443\nGNUNE_SYNC_ORIGIN=https://192.0.2.1:8443\nGNUNE_SYNC_CERT=certificates/current/server-chain.pem\nGNUNE_SYNC_KEY=certificates/current/server-key.pem").unwrap();
        let connection = parse_connection(&values).unwrap();
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            connection.cert,
            cwd.join("certificates/current/server-chain.pem")
        );
        assert_eq!(
            connection.key,
            cwd.join("certificates/current/server-key.pem")
        );
        let encoded = serde_json::to_vec(&connection).unwrap();
        let restored: Connection = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(restored.cert, connection.cert);
        assert_eq!(restored.key, connection.key);
    }
    #[test]
    fn consent_roundtrip_keeps_queue_separate_from_playback_and_can_stop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sharing.json");
        assert!(load_at(&path).unwrap().is_none());
        let connection = parse_connection(&literal_env("GNUNE_SYNC_BIND=192.0.2.1:8443\nGNUNE_SYNC_ORIGIN=https://192.0.2.1:8443\nGNUNE_SYNC_CERT=/tmp/cert\nGNUNE_SYNC_KEY=/tmp/key").unwrap()).unwrap();
        let mut saved = Saved {
            enabled: true,
            connection,
            mode: ShareMode::Queue,
            include_video: false,
            include_podcasts: false,
            queue: Some(Grant {
                scope: "only-this-queue".into(),
                ..Grant::default()
            }),
            skipped: 0,
        };
        save_at(&path, &saved).unwrap();
        assert_eq!(
            load_at(&path).unwrap().unwrap().queue.unwrap().scope,
            "only-this-queue"
        );
        saved.enabled = false;
        save_at(&path, &saved).unwrap();
        assert!(!load_at(&path).unwrap().unwrap().enabled);
        fs::write(&path, "corrupt").unwrap();
        assert!(load_at(&path).is_err());
    }
    #[test]
    fn malformed_tls_and_mismatched_keys_are_rejected() {
        use openssl::{
            asn1::Asn1Time,
            hash::MessageDigest,
            pkey::PKey,
            rsa::Rsa,
            x509::{X509, X509NameBuilder},
        };
        let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
        let other = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_text("CN", "test").unwrap();
        let name = name.build();
        let mut cert = X509::builder().unwrap();
        cert.set_version(2).unwrap();
        cert.set_subject_name(&name).unwrap();
        cert.set_issuer_name(&name).unwrap();
        cert.set_pubkey(&key).unwrap();
        cert.set_not_before(&Asn1Time::days_from_now(0).unwrap())
            .unwrap();
        cert.set_not_after(&Asn1Time::days_from_now(1).unwrap())
            .unwrap();
        cert.sign(&key, MessageDigest::sha256()).unwrap();
        let cert = cert.build().to_pem().unwrap();
        assert!(validate_tls(&cert, &key.private_key_to_pem_pkcs8().unwrap()).is_ok());
        assert!(
            validate_tls(&cert, &other.private_key_to_pem_pkcs8().unwrap())
                .unwrap_err()
                .contains("do not match")
        );
        assert!(validate_tls(b"broken", b"broken").is_err());
    }
}

#[cfg(test)]
mod unix_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn unix_listener_is_private_and_rejects_an_occupied_path() {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.path().join("api.sock");
        let server = unix_server(&path).unwrap();
        assert!(server.server_addr().to_ip().is_none());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(unix_server(&path).is_err());
        drop(server);
        assert!(!path.exists());
        fs::write(&path, "keep this file").unwrap();
        assert!(unix_server(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "keep this file");
    }

    #[test]
    fn unix_listener_refuses_a_directory_accessible_to_other_users() {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            unix_server(&directory.path().join("api.sock"))
                .err()
                .unwrap()
                .contains("0700")
        );
    }

    #[test]
    fn saved_unix_endpoint_keeps_tls_fallback_and_defaults_old_config_to_native() {
        let legacy = r#"{"bind":"127.0.0.1:8443","origin":"https://fixture.example.ts.net:8443","cert":"/fallback/cert.pem","key":"/fallback/key.pem"}"#;
        let mut connection: Connection = serde_json::from_str(legacy).unwrap();
        assert!(connection.serve_socket.is_none());
        connection.serve_socket = Some("/private/gnune/api.sock".into());
        let roundtrip: Connection =
            serde_json::from_str(&serde_json::to_string(&connection).unwrap()).unwrap();
        assert_eq!(roundtrip.cert, connection.cert);
        assert_eq!(roundtrip.key, connection.key);
        assert_eq!(roundtrip.serve_socket, connection.serve_socket);
    }
}
