//! Remote-control listener for companion clients (phone, tablet).
//!
//! The local automation API is enough for the CLI, a kitten, or the MCP server:
//! it binds loopback and mints a fresh token on every launch, so nothing
//! outside this machine can reach it. A paired device needs the opposite — a
//! stable address and a token that survives restarts — so this module adds an
//! opt-in second listener on `terminal.remote_control`.
//!
//! Reachability is expected to come from the network layer (Tailscale,
//! WireGuard, or the LAN), so the listener speaks plain HTTP. Every request
//! still has to carry the bearer token stored in the data directory, and the
//! token can be rotated to unpair every device at once.

use std::{
    fs,
    io::Write as _,
    net::{IpAddr, UdpSocket},
    path::{Path, PathBuf},
};

use futures::channel::mpsc;
use gpui::{App, Global};
use session::ipc_server::{IpcRequest, RemoteServer};
use settings::Settings as _;
use terminal::terminal_settings::{RemoteControlSettings, TerminalSettings};

/// File in the data directory holding the paired-device bearer token.
const TOKEN_FILE: &str = "remote-token";

/// Addresses probed to discover the IP this machine uses on each network.
///
/// `100.100.100.100` is Tailscale's MagicDNS address, and `192.0.2.1` is
/// TEST-NET-1, which routes out of the default interface and never answers.
/// `UdpSocket::connect` only resolves the route, so no packet is sent.
const PROBE_TARGETS: [&str; 2] = ["100.100.100.100", "192.0.2.1"];

/// The request channel shared with the loopback listener.
struct RequestSender(mpsc::UnboundedSender<IpcRequest>);

impl Global for RequestSender {}

#[derive(Default)]
struct RemoteControlState {
    /// Settings the current listener was started from, so an unrelated
    /// settings change does not restart it.
    applied: Option<RemoteControlSettings>,
    running: Option<RemoteServer>,
    /// URLs a client can use to reach this machine.
    urls: Vec<String>,
    error: Option<String>,
}

impl Global for RemoteControlState {}

/// What the settings page shows about the endpoint.
pub struct RemoteControlStatus {
    pub enabled: bool,
    /// URLs a client can use to reach this machine.
    pub urls: Vec<String>,
    pub token: String,
    pub error: Option<String>,
}

/// Installs the request channel and starts the listener when it is enabled.
pub fn init(cx: &mut App, requests: mpsc::UnboundedSender<IpcRequest>) {
    cx.set_global(RequestSender(requests));
    cx.set_global(RemoteControlState::default());
    apply(cx);

    // Settings change from this window, from `settings.json` edited by hand, or
    // from the settings watcher. `apply` is a no-op unless the endpoint
    // configuration actually changed.
    cx.observe_global::<settings::SettingsStore>(|cx| apply(cx))
        .detach();
}

/// Starts, restarts, or stops the listener so it matches the current settings.
pub fn apply(cx: &mut App) {
    let config = TerminalSettings::get_global(cx).remote_control.clone();
    let unchanged = cx
        .try_global::<RemoteControlState>()
        .and_then(|state| state.applied.as_ref())
        .is_some_and(|applied| *applied == config);
    if unchanged {
        return;
    }
    apply_config(cx, &config);
}

/// Applies `config` immediately, without waiting for the settings file to
/// reload. The caller is still responsible for persisting it.
pub fn apply_config(cx: &mut App, config: &RemoteControlSettings) {
    let Some(requests) = cx
        .try_global::<RequestSender>()
        .map(|sender| sender.0.clone())
    else {
        return;
    };

    let mut next = RemoteControlState {
        applied: Some(config.clone()),
        ..Default::default()
    };

    if config.enabled {
        let token = load_or_create_token();
        match session::ipc_server::start_remote_server(requests, &config.bind, config.port, token) {
            Ok(server) => {
                next.urls = endpoint_urls(&config.bind, server.port());
                next.running = Some(server);
            }
            Err(error) => next.error = Some(error),
        }
    }

    // Replacing the state drops the previous `RemoteServer`, which signals its
    // graceful shutdown.
    cx.set_global(next);
    cx.refresh_windows();
}

/// Enables or disables the endpoint and applies the change right away.
pub fn set_enabled(cx: &mut App, enabled: bool) {
    let mut config = TerminalSettings::get_global(cx).remote_control.clone();
    config.enabled = enabled;
    apply_config(cx, &config);
}

/// Replaces the token, so every paired device has to pair again.
pub fn rotate_token(cx: &mut App) {
    if let Some(parent) = token_path().parent() {
        fs::create_dir_all(parent).ok();
    }
    write_private(&token_path(), &uuid::Uuid::new_v4().to_string());

    if cx.try_global::<RemoteControlState>().is_some() {
        cx.global_mut::<RemoteControlState>().applied = None;
    }
    apply(cx);
}

/// Current endpoint configuration plus live state, for the settings page.
pub fn status(cx: &App) -> RemoteControlStatus {
    let config = TerminalSettings::get_global(cx).remote_control.clone();
    let state = cx.try_global::<RemoteControlState>();
    let applied = state.and_then(|state| state.applied.as_ref());

    RemoteControlStatus {
        // The listener is the source of truth right after a toggle, before the
        // settings file has been rewritten and reloaded.
        enabled: applied.map_or(config.enabled, |applied| applied.enabled),
        urls: state.map(|state| state.urls.clone()).unwrap_or_default(),
        token: read_token().unwrap_or_default(),
        error: state.and_then(|state| state.error.clone()),
    }
}

fn token_path() -> PathBuf {
    paths::data_dir().join(TOKEN_FILE)
}

fn read_token() -> Option<String> {
    read_token_at(&token_path())
}

fn read_token_at(path: &Path) -> Option<String> {
    let token = fs::read_to_string(path).ok()?;
    let token = token.trim().to_string();
    (!token.is_empty()).then_some(token)
}

/// Reads the paired-device token, creating one on first use.
fn load_or_create_token() -> String {
    load_or_create_token_at(&token_path())
}

fn load_or_create_token_at(path: &Path) -> String {
    if let Some(token) = read_token_at(path) {
        return token;
    }

    let token = uuid::Uuid::new_v4().to_string();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    write_private(path, &token);
    token
}

/// Writes a file that only the current user can read.
fn write_private(path: &Path, contents: &str) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

        if let Ok(mut file) = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
        {
            let _ = file.write_all(contents.as_bytes());
            let _ = file.flush();
        }
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }

    #[cfg(not(unix))]
    {
        let _ = fs::write(path, contents);
    }
}

/// URLs a paired device can use for a listener bound to `bind`.
fn endpoint_urls(bind: &str, port: u16) -> Vec<String> {
    let hosts = match bind.parse::<IpAddr>() {
        Ok(address) if address.is_unspecified() => discover_addresses(),
        Ok(address) => vec![address],
        // A hostname such as `localhost` is handed to the client verbatim.
        Err(_) => return vec![format!("http://{}:{port}", host_for_url(bind))],
    };

    let mut urls: Vec<String> = hosts
        .into_iter()
        .map(|address| format!("http://{}:{port}", host_for_url(&address.to_string())))
        .collect();

    let loopback = format!("http://127.0.0.1:{port}");
    if !urls.contains(&loopback) {
        urls.push(loopback);
    }
    urls
}

/// IPv6 hosts need brackets inside a URL.
fn host_for_url(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

/// Non-loopback addresses this machine uses, tailnet address first.
fn discover_addresses() -> Vec<IpAddr> {
    let mut addresses = Vec::new();
    for target in PROBE_TARGETS {
        if let Some(address) = probe_local_address(target)
            && !addresses.contains(&address)
        {
            addresses.push(address);
        }
    }
    addresses
}

fn probe_local_address(target: &str) -> Option<IpAddr> {
    let socket = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    socket.connect((target, 9)).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(address) if !address.is_unspecified() && !address.is_loopback() => {
            Some(IpAddr::V4(address))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unspecified_bind_lists_loopback_and_discovered_addresses() {
        let urls = endpoint_urls("0.0.0.0", 47823);

        assert!(
            urls.contains(&"http://127.0.0.1:47823".to_string()),
            "loopback must always be offered: {urls:?}"
        );
        assert!(
            !urls.contains(&"http://0.0.0.0:47823".to_string()),
            "0.0.0.0 is not a usable host: {urls:?}"
        );
    }

    #[test]
    fn a_specific_bind_address_is_used_verbatim() {
        let urls = endpoint_urls("100.101.102.103", 47823);

        assert_eq!(
            urls.first().map(String::as_str),
            Some("http://100.101.102.103:47823")
        );
        assert!(urls.contains(&"http://127.0.0.1:47823".to_string()));
    }

    #[test]
    fn ipv6_hosts_are_bracketed() {
        assert_eq!(host_for_url("fd7a:115c::1"), "[fd7a:115c::1]");
        assert_eq!(host_for_url("100.1.2.3"), "100.1.2.3");
    }

    #[test]
    fn hostnames_pass_through() {
        assert_eq!(
            endpoint_urls("my-laptop.tailnet.ts.net", 47823),
            vec!["http://my-laptop.tailnet.ts.net:47823".to_string()]
        );
    }

    #[test]
    fn token_is_created_once_and_reused() {
        let dir = std::env::temp_dir().join(format!(
            "terry-remote-token-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let path = dir.join(TOKEN_FILE);

        let first = load_or_create_token_at(&path);
        assert!(!first.is_empty());
        assert_eq!(load_or_create_token_at(&path), first);
        assert_eq!(read_token_at(&path).as_deref(), Some(first.as_str()));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(&path)
                .expect("token file")
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "the token grants shell access, so only the user may read it"
            );
        }

        fs::remove_dir_all(&dir).ok();
    }
}
