//! SSH host discovery and tmux session helpers for the terminal pickers.
//!
//! The pickers do not implement SSH or tmux themselves: they build an `ssh` /
//! `tmux` command line and launch it as a normal terminal program. That keeps
//! behaviour identical to a classic terminal (ssh_config applies, tmux talks to
//! the same server) and means session restore only has to replay the argv.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use terminal::terminal_settings::{TerminalSettings, TerminalSshHost};

/// A connectable SSH host, whether discovered from `~/.ssh/config`, defined in
/// settings, or picked from recents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshTarget {
    /// Text shown in the picker.
    pub label: String,
    /// Argument passed to `ssh` (an alias, so ssh_config still applies).
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    /// `HostName` from ssh_config, shown as secondary text when it differs from
    /// the alias.
    pub host_name: Option<String>,
}

impl SshTarget {
    pub fn ad_hoc(host: &str) -> Self {
        Self {
            label: host.to_string(),
            host: host.to_string(),
            user: None,
            port: None,
            identity_file: None,
            host_name: None,
        }
    }

    /// `user@host`, or just `host` when no user is configured. A host that
    /// already contains `@` is left alone.
    pub fn login(&self) -> String {
        match &self.user {
            Some(user) if !self.host.contains('@') => format!("{user}@{}", self.host),
            _ => self.host.clone(),
        }
    }

    /// Detail line for the picker, e.g. `alice@example.com:2222`.
    pub fn detail(&self) -> String {
        let mut text = self.login();
        if let Some(port) = self.port {
            text.push(':');
            text.push_str(&port.to_string());
        }
        if let Some(host_name) = self.host_name.as_deref() {
            if host_name != self.host && !text.contains(host_name) {
                text.push_str("  ·  ");
                text.push_str(host_name);
            }
        }
        text
    }
}

/// Parses `~/.ssh/config` into hosts. Wildcard-only blocks (`Host *`) and
/// `Match` blocks are skipped because they are not connectable targets. Later
/// directives only fill unset fields, matching ssh's first-value-wins rule.
pub fn parse_ssh_config(contents: &str) -> Vec<SshTarget> {
    let mut hosts: Vec<SshTarget> = Vec::new();
    let mut active: Vec<usize> = Vec::new();

    for line in contents.lines() {
        // `Host alias1 alias2` / `Hostname x` / `Port 22 ...`.
        let Some((key, value)) = split_directive(line) else {
            continue;
        };
        match key.to_ascii_lowercase().as_str() {
            "host" => {
                active.clear();
                for alias in value.split_whitespace() {
                    if alias.starts_with('!')
                        || alias.starts_with('#')
                        || alias.contains('*')
                        || alias.contains('?')
                    {
                        continue;
                    }
                    let alias = alias.to_string();
                    if hosts.iter().any(|host| host.host == alias) {
                        continue;
                    }
                    hosts.push(SshTarget {
                        label: alias.clone(),
                        host: alias,
                        user: None,
                        port: None,
                        identity_file: None,
                        host_name: None,
                    });
                    active.push(hosts.len() - 1);
                }
            }
            "match" => active.clear(),
            "hostname" => {
                for &index in &active {
                    if hosts[index].host_name.is_none() {
                        hosts[index].host_name = Some(value.to_string());
                    }
                }
            }
            "user" => {
                for &index in &active {
                    if hosts[index].user.is_none() {
                        hosts[index].user = Some(value.to_string());
                    }
                }
            }
            "port" => {
                if let Ok(port) = value.parse::<u16>() {
                    for &index in &active {
                        if hosts[index].port.is_none() {
                            hosts[index].port = Some(port);
                        }
                    }
                }
            }
            "identityfile" => {
                for &index in &active {
                    if hosts[index].identity_file.is_none() {
                        hosts[index].identity_file = Some(value.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    hosts
}

/// Splits a config line into a directive key and its value. Returns `None` for
/// blank lines and comments.
fn split_directive(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (key, value) = line.split_once(|c: char| c.is_whitespace() || c == '=')?;
    let value = value.trim_start_matches(['=', ' ', '\t']).trim();
    Some((key.trim(), value))
}

/// Reads `~/.ssh/config` (following `Include` directives), returning an empty
/// list when it does not exist.
pub fn ssh_config_hosts() -> Vec<SshTarget> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let main = home.join(".ssh").join("config");
    if !main.is_file() {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    let text = read_ssh_config_text(&main, 0, &mut seen);
    parse_ssh_config(&text)
}

/// Expands `Include` directives in place so ssh's first-value-wins ordering is
/// preserved. Paths may be absolute, relative to the including file, start with
/// `~/`, and contain simple `*` / `?` globs in the final component. Cycles are
/// broken with `seen`, and nesting is bounded.
fn read_ssh_config_text(path: &Path, depth: usize, seen: &mut HashSet<PathBuf>) -> String {
    if depth > 8 {
        return String::new();
    }
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !seen.insert(canonical) {
        return String::new();
    }
    let Ok(contents) = std::fs::read_to_string(path) else {
        return String::new();
    };

    let mut out = String::with_capacity(contents.len());
    for line in contents.lines() {
        match split_directive(line) {
            Some((key, value)) if key.eq_ignore_ascii_case("include") => {
                for pattern in value.split_whitespace() {
                    for included in expand_include(pattern, path) {
                        out.push_str(&read_ssh_config_text(&included, depth + 1, seen));
                        out.push('\n');
                    }
                }
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

fn expand_include(pattern: &str, including_file: &Path) -> Vec<PathBuf> {
    let expanded = if let Some(rest) = pattern.strip_prefix("~/") {
        match dirs::home_dir() {
            Some(home) => home.join(rest),
            None => return Vec::new(),
        }
    } else if Path::new(pattern).is_absolute() {
        PathBuf::from(pattern)
    } else {
        including_file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(pattern)
    };

    let file_pattern = expanded
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    if !file_pattern.contains(['*', '?']) {
        return vec![expanded];
    }

    let parent = expanded
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut matches = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            path.file_name()
                .map(|name| wildcard_match(&file_pattern, &name.to_string_lossy()))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    matches.sort();
    matches
}

/// Minimal glob matcher supporting `*` and `?`, which covers `Include` use.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let text = text.chars().collect::<Vec<_>>();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<usize> = None;
    let mut star_text = 0;

    while ti < text.len() {
        if pi < pattern.len() && (pattern[pi] == '?' || pattern[pi] == text[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pattern.len() && pattern[pi] == '*' {
            star = Some(pi);
            star_text = ti;
            pi += 1;
        } else if let Some(star_index) = star {
            pi = star_index + 1;
            star_text += 1;
            ti = star_text;
        } else {
            return false;
        }
    }

    while pi < pattern.len() && pattern[pi] == '*' {
        pi += 1;
    }
    pi == pattern.len()
}

/// Options that shape every `ssh` invocation.
#[derive(Clone, Copy, Debug)]
pub struct SshOptions<'a> {
    /// Share one connection to the same host for 60s (`ControlMaster`).
    pub reuse_connections: bool,
    /// Extra user-provided arguments inserted before the destination.
    pub extra_args: &'a [String],
}

/// Builds the `ssh` command for `target`. `remote_tmux` attaches to (or creates)
/// a tmux session on the remote host, which is the closest thing to a remote
/// mux without implementing tmux's control protocol.
pub fn ssh_invocation(
    target: &SshTarget,
    options: SshOptions<'_>,
    remote_tmux: Option<&str>,
) -> (String, Vec<String>, String) {
    let mut args = Vec::new();

    if options.reuse_connections && cfg!(unix) {
        args.push("-o".to_string());
        args.push("ControlMaster=auto".to_string());
        args.push("-o".to_string());
        args.push(format!("ControlPath={}", control_path()));
        args.push("-o".to_string());
        args.push("ControlPersist=60s".to_string());
    }
    if let Some(port) = target.port {
        args.push("-p".to_string());
        args.push(port.to_string());
    }
    if let Some(identity_file) = &target.identity_file {
        args.push("-i".to_string());
        // argv is not shell-expanded, so expand `~` ourselves.
        args.push(expand_tilde(identity_file));
    }
    args.extend(options.extra_args.iter().cloned());

    if remote_tmux.is_some() {
        // Allocate a TTY so tmux can attach.
        args.push("-t".to_string());
    }
    args.push(target.login());

    let title = match remote_tmux {
        Some(session) => format!("{} · {}", target.label, session),
        None => target.label.clone(),
    };

    if let Some(session) = remote_tmux {
        args.push("tmux".to_string());
        args.extend(local_tmux_args(session));
    }

    ("ssh".to_string(), args, title)
}

/// Expands a leading `~/` (or a bare `~`) because arguments passed to `ssh`
/// directly are not run through a shell.
fn expand_tilde(path: &str) -> String {
    let rest = match path {
        "~" => Some(""),
        _ => path.strip_prefix("~/"),
    };
    match (rest, dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest).to_string_lossy().to_string(),
        _ => path.to_string(),
    }
}

/// A control socket path. `%C` is expanded by ssh to a hash of the connection
/// tuple, so every host gets its own socket. The pid namespaces sockets per app
/// run while still allowing the 60s `ControlPersist` window to be shared by all
/// terminals opened from this window.
fn control_path() -> String {
    let dir = std::env::temp_dir();
    dir.join(format!("terry-ssh-{}-%C", std::process::id()))
        .to_string_lossy()
        .to_string()
}

/// A local tmux session as reported by `tmux list-sessions`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxSession {
    pub name: String,
    pub windows: Option<usize>,
    pub attached: bool,
}

impl TmuxSession {
    pub fn detail(&self) -> String {
        let mut parts = Vec::new();
        if let Some(windows) = self.windows {
            parts.push(format!(
                "{windows} window{}",
                if windows == 1 { "" } else { "s" }
            ));
        }
        if self.attached {
            parts.push("attached".to_string());
        }
        parts.join(" · ")
    }
}

/// Lists local tmux sessions synchronously. Returns an empty list when tmux is
/// not installed or no server is running (both are normal states).
pub fn list_tmux_sessions() -> Vec<TmuxSession> {
    let output = Command::new("tmux")
        .args([
            "list-sessions",
            "-F",
            "#{session_name}\t#{session_windows}\t#{session_attached}",
        ])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut sessions = Vec::new();
    for line in text.lines() {
        let mut fields = line.split('\t');
        let Some(name) = fields.next().filter(|name| !name.is_empty()) else {
            continue;
        };
        sessions.push(TmuxSession {
            name: name.to_string(),
            windows: fields.next().and_then(|n| n.parse().ok()),
            attached: fields.next().is_some_and(|flag| flag.trim() == "1"),
        });
    }
    sessions.sort_by(|a, b| a.name.cmp(&b.name));
    sessions
}

/// Arguments for `tmux` that attach to `session`, creating it when missing.
pub fn local_tmux_args(session: &str) -> Vec<String> {
    vec![
        "new-session".to_string(),
        "-A".to_string(),
        "-s".to_string(),
        session.to_string(),
    ]
}

/// `(program, args, title)` for a local tmux session terminal.
pub fn local_tmux_invocation(session: &str) -> (String, Vec<String>, String) {
    (
        "tmux".to_string(),
        local_tmux_args(session),
        format!("tmux: {session}"),
    )
}

/// Turns a host or group label into a valid tmux session name.
pub fn tmux_session_name(label: &str) -> String {
    let mut name = String::with_capacity(label.len());
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
            name.push(ch);
        } else if ch == '.' || ch == ' ' {
            name.push('-');
        }
    }
    let name = name.trim_matches('-').to_string();
    if name.is_empty() {
        "main".to_string()
    } else {
        name
    }
}

/// Every host the SSH picker offers: settings-defined hosts first, then
/// `~/.ssh/config`, then recents that are not already present.
pub fn ssh_targets(settings: &TerminalSettings) -> Vec<SshTarget> {
    let mut targets: Vec<SshTarget> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for host in &settings.ssh_hosts {
        push_unique(&mut targets, &mut seen, from_settings(host));
    }
    for host in ssh_config_hosts() {
        push_unique(&mut targets, &mut seen, host);
    }
    for host in load_recent_hosts() {
        push_unique(&mut targets, &mut seen, SshTarget::ad_hoc(&host));
    }
    targets
}

fn from_settings(host: &TerminalSshHost) -> SshTarget {
    SshTarget {
        label: host.name.clone(),
        host: host.host.clone(),
        user: host.user.clone(),
        port: host.port,
        identity_file: host.identity_file.clone(),
        host_name: None,
    }
}

fn push_unique(targets: &mut Vec<SshTarget>, seen: &mut HashSet<String>, host: SshTarget) {
    let key = host.host.to_ascii_lowercase();
    if seen.insert(key) {
        targets.push(host);
    }
}

fn recent_hosts_path() -> PathBuf {
    paths::state_dir().join("ssh_recent_hosts.json")
}

/// Recently connected hosts, most recent first.
pub fn load_recent_hosts() -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(recent_hosts_path()) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// Records `host` as recently used. Failures are ignored: recents are a
/// convenience, never a correctness requirement.
pub fn record_recent_host(host: &str) {
    const LIMIT: usize = 10;
    let mut recents = load_recent_hosts();
    recents.retain(|existing| existing != host);
    recents.insert(0, host.to_string());
    recents.truncate(LIMIT);
    let path = recent_hosts_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        path,
        serde_json::to_string_pretty(&recents).unwrap_or_default(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hosts_with_options_and_skips_wildcards() {
        let config = r#"
# comment
Host web
    HostName web.example.com
    User alice
    Port 2222
    IdentityFile ~/.ssh/id_web

Host db staging
    User bob

Host *
    User ignored

Match host foo
    User nobody
"#;
        let hosts = parse_ssh_config(config);
        assert_eq!(hosts.len(), 3);
        assert_eq!(hosts[0].host, "web");
        assert_eq!(hosts[0].host_name.as_deref(), Some("web.example.com"));
        assert_eq!(hosts[0].user.as_deref(), Some("alice"));
        assert_eq!(hosts[0].port, Some(2222));
        assert_eq!(hosts[0].login(), "alice@web");
        assert_eq!(hosts[1].host, "db");
        assert_eq!(hosts[2].host, "staging");
        assert_eq!(hosts[2].user.as_deref(), Some("bob"));
    }

    #[test]
    fn first_value_wins_for_duplicate_directives() {
        let hosts =
            parse_ssh_config("Host box\n  User first\n  User second\n  Port 2200\n  Port 2300\n");
        assert_eq!(hosts[0].user.as_deref(), Some("first"));
        assert_eq!(hosts[0].port, Some(2200));
    }

    #[test]
    fn wildcard_matching_supports_star_and_question() {
        assert!(wildcard_match("*.conf", "web.conf"));
        assert!(wildcard_match("web?conf", "web.conf"));
        assert!(wildcard_match("*", "anything"));
        assert!(!wildcard_match("*.conf", "web.toml"));
        assert!(!wildcard_match("web?conf", "web.confx"));
    }

    #[test]
    fn include_directives_are_expanded_in_place() {
        let dir =
            std::env::temp_dir().join(format!("terry-ssh-include-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("config.d")).unwrap();
        std::fs::write(
            dir.join("config"),
            "Host main\n  User a\nInclude config.d/*.conf\nHost tail\n  User b\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("config.d").join("20-db.conf"),
            "Host db\n  HostName db.example.com\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("config.d").join("10-web.conf"),
            "Host web\n  User c\n",
        )
        .unwrap();

        let mut seen = HashSet::new();
        let text = read_ssh_config_text(&dir.join("config"), 0, &mut seen);
        let hosts = parse_ssh_config(&text);
        let names = hosts
            .iter()
            .map(|host| host.host.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["main", "web", "db", "tail"]);
        assert_eq!(hosts[1].user.as_deref(), Some("c"));
        assert_eq!(hosts[2].host_name.as_deref(), Some("db.example.com"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn builds_ssh_args_with_reuse_and_tmux() {
        let extra = Vec::new();
        let options = SshOptions {
            reuse_connections: true,
            extra_args: &extra,
        };
        let target = SshTarget {
            label: "web".into(),
            host: "web".into(),
            user: Some("alice".into()),
            port: Some(2222),
            identity_file: Some("~/.ssh/id_web".into()),
            host_name: None,
        };
        let (program, args, title) = ssh_invocation(&target, options, Some("main"));
        assert_eq!(program, "ssh");
        assert_eq!(title, "web · main");
        assert!(args.contains(&"alice@web".to_string()));
        assert!(args.contains(&"2222".to_string()));
        assert!(args.contains(&"-i".to_string()));
        assert!(args.iter().any(|arg| arg.ends_with(".ssh/id_web")));
        assert!(args.ends_with(&[
            "alice@web".to_string(),
            "tmux".to_string(),
            "new-session".to_string(),
            "-A".to_string(),
            "-s".to_string(),
            "main".to_string(),
        ]));
        #[cfg(unix)]
        assert!(args.iter().any(|arg| arg.starts_with("ControlPath=")));
        #[cfg(unix)]
        assert!(args.contains(&"-t".to_string()));
    }

    #[test]
    fn ad_hoc_hosts_do_not_duplicate_users() {
        let target = SshTarget {
            user: Some("alice".into()),
            ..SshTarget::ad_hoc("bob@example.com")
        };
        assert_eq!(target.login(), "bob@example.com");
    }

    #[test]
    fn tmux_names_are_sanitized() {
        assert_eq!(tmux_session_name("web.example.com"), "web-example-com");
        assert_eq!(tmux_session_name("my server"), "my-server");
        assert_eq!(tmux_session_name("***"), "main");
        assert_eq!(tmux_session_name(""), "main");
    }
}
