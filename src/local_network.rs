//! Local Network privacy handling for SSH connections on macOS.
//!
//! macOS records the Local Network privilege per application. A child `ssh`
//! process is attributed to Terry, but the denial can otherwise surface only as
//! a generic "No route to host". Probe the privilege before launching SSH so
//! macOS can present its permission alert while the user is still in context.

use std::net::{IpAddr, Ipv4Addr};

use crate::remote_hosts::SshTarget;

pub const SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum LocalNetworkPermission {
    /// The platform or target does not use the Local Network privilege.
    NotRequired,
    Granted,
    Denied,
    /// The probe could not determine the privilege. SSH should still be allowed
    /// to proceed; it may trigger the system alert itself.
    Unknown,
}

/// Whether connecting to this SSH target may require the macOS Local Network
/// privilege. Public internet hosts and loopback addresses bypass the probe.
pub fn ssh_target_may_need_local_network(target: &SshTarget) -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }

    let host = target.host_name.as_deref().unwrap_or(&target.host);
    host_may_be_local(host)
}

/// Trigger and observe the macOS Local Network privilege. On other platforms
/// the privilege does not apply.
pub fn probe_permission() -> LocalNetworkPermission {
    #[cfg(target_os = "macos")]
    {
        macos::probe_permission()
    }

    #[cfg(not(target_os = "macos"))]
    {
        LocalNetworkPermission::NotRequired
    }
}

fn host_may_be_local(raw_host: &str) -> bool {
    let host = normalize_host(raw_host);
    if host.is_empty() {
        return false;
    }

    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") {
        return false;
    }

    if let Ok(address) = host.parse::<IpAddr>() {
        return ip_may_be_local(address);
    }

    const LOCAL_SUFFIXES: &[&str] = &[
        ".local",
        ".localdomain",
        ".lan",
        ".home",
        ".internal",
        ".intranet",
    ];
    if LOCAL_SUFFIXES.iter().any(|suffix| lower.ends_with(suffix)) {
        return true;
    }

    // ssh_config aliases often have no dot. Resolving them could block the UI,
    // and treating them as potentially local is safer than silently skipping
    // the permission request.
    !host.contains('.')
}

fn normalize_host(raw_host: &str) -> &str {
    let host = raw_host.trim();
    let host = host.rsplit_once('@').map_or(host, |(_, host)| host).trim();

    if let Some(rest) = host.strip_prefix('[')
        && let Some(end) = rest.find(']')
    {
        return &rest[..end];
    }

    if host.parse::<IpAddr>().is_ok() {
        return host.trim_end_matches('.');
    }

    if let Some((host, port)) = host.rsplit_once(':')
        && !host.is_empty()
        && port.parse::<u16>().is_ok()
    {
        return host.trim_end_matches('.');
    }

    host.trim_end_matches('.')
}

fn ip_may_be_local(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            !address.is_loopback()
                && !address.is_unspecified()
                && (address.is_private()
                    || address.is_link_local()
                    || address.is_multicast()
                    || ipv4_is_shared(address))
        }
        IpAddr::V6(address) => {
            !address.is_loopback()
                && !address.is_unspecified()
                && (address.is_unique_local()
                    || address.is_unicast_link_local()
                    || address.is_multicast())
        }
    }
}

fn ipv4_is_shared(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 100 && (64..=127).contains(&octets[1])
}

#[cfg(target_os = "macos")]
mod macos {
    use std::{ffi::c_void, ptr, sync::mpsc, time::Duration};

    use block2::RcBlock;
    use dispatch2::{DispatchQueue, DispatchRetained};

    use super::LocalNetworkPermission;

    type NwObject = *mut c_void;

    const STATE_READY: i32 = 1;
    const STATE_FAILED: i32 = 2;
    const STATE_WAITING: i32 = 4;
    const POLICY_DENIED: i32 = -65570;
    const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

    #[link(name = "Network", kind = "framework")]
    unsafe extern "C" {
        fn nw_browse_descriptor_create_bonjour_service(
            service_type: *const std::ffi::c_char,
            domain: *const std::ffi::c_char,
        ) -> NwObject;
        fn nw_parameters_create() -> NwObject;
        fn nw_browser_create(descriptor: NwObject, parameters: NwObject) -> NwObject;
        fn nw_browser_set_queue(browser: NwObject, queue: *mut c_void);
        fn nw_browser_set_state_changed_handler(browser: NwObject, handler: *mut c_void);
        fn nw_browser_start(browser: NwObject);
        fn nw_browser_cancel(browser: NwObject);
        fn nw_error_get_error_code(error: NwObject) -> i32;
        fn nw_release(object: NwObject);
    }

    pub(super) fn probe_permission() -> LocalNetworkPermission {
        // Browsing a declared Bonjour service type is a local network operation.
        // The system either grants it, waits for the user's alert response, or
        // reports DNS-SD's PolicyDenied error.
        let descriptor = unsafe {
            nw_browse_descriptor_create_bonjour_service(c"_ssh._tcp".as_ptr(), ptr::null())
        };
        if descriptor.is_null() {
            return LocalNetworkPermission::Unknown;
        }

        let parameters = unsafe { nw_parameters_create() };
        let browser = unsafe { nw_browser_create(descriptor, parameters) };
        if browser.is_null() {
            unsafe {
                release(descriptor);
                release(parameters);
            }
            return LocalNetworkPermission::Unknown;
        }

        let (sender, receiver) = mpsc::sync_channel(1);
        let handler = RcBlock::new(move |state: i32, error: NwObject| {
            let permission = match state {
                STATE_READY => Some(LocalNetworkPermission::Granted),
                STATE_WAITING if is_policy_denied(error) => Some(LocalNetworkPermission::Denied),
                STATE_FAILED if is_policy_denied(error) => Some(LocalNetworkPermission::Denied),
                STATE_FAILED => Some(LocalNetworkPermission::Unknown),
                _ => None,
            };
            if let Some(permission) = permission {
                sender.try_send(permission).ok();
            }
        });

        let queue = DispatchQueue::new("dev.terry.local-network-permission", None);
        unsafe {
            nw_browser_set_queue(
                browser,
                DispatchRetained::as_ptr(&queue).as_ptr().cast::<c_void>(),
            );
            nw_browser_set_state_changed_handler(browser, RcBlock::as_ptr(&handler).cast());
            nw_browser_start(browser);
        }

        let permission = receiver
            .recv_timeout(PROBE_TIMEOUT)
            .unwrap_or(LocalNetworkPermission::Unknown);

        unsafe {
            nw_browser_cancel(browser);
            release(browser);
            release(descriptor);
            release(parameters);
        }

        permission
    }

    fn is_policy_denied(error: NwObject) -> bool {
        !error.is_null() && unsafe { nw_error_get_error_code(error) } == POLICY_DENIED
    }

    unsafe fn release(object: NwObject) {
        if !object.is_null() {
            unsafe { nw_release(object) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::host_may_be_local;

    #[test]
    fn recognizes_local_network_hosts() {
        for host in [
            "192.168.1.10",
            "10.0.0.2",
            "172.16.4.3",
            "169.254.1.1",
            "100.64.0.1",
            "fe80::1",
            "fd00::1",
            "server.local",
            "server.local.",
            "nas.lan",
            "workstation",
            "alice@workstation",
            "[fe80::1]:22",
        ] {
            assert!(host_may_be_local(host), "{host} should be treated as local");
        }
    }

    #[test]
    fn ignores_public_and_loopback_hosts() {
        for host in [
            "example.com",
            "8.8.8.8",
            "2001:4860:4860::8888",
            "localhost",
            "127.0.0.1",
            "::1",
        ] {
            assert!(
                !host_may_be_local(host),
                "{host} should not trigger the local network probe"
            );
        }
    }
}
