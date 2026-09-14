use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result};
use collections::HashMap;
use futures::channel::oneshot;
use serde::Deserialize;

use crate::shell::ShellKind;

static SHELL_ENV_LOADED: AtomicBool = AtomicBool::new(false);
static SHELL_ENV_WAITERS: Mutex<Vec<oneshot::Sender<()>>> = Mutex::new(Vec::new());

/// Environment variables that launchers (IDEs, AI agent shells, process
/// managers) commonly inject to disable interactive pagers. They must not leak
/// into Terry's process or terminal environments: terminal shells re-read the
/// user's dotfiles, so a genuine pager configuration still takes effect, while
/// commands like `git log` keep paging through `less` with space to scroll,
/// matching standalone terminal emulators.
pub const PAGER_OVERRIDE_ENV_VARS: &[&str] = &["GIT_PAGER", "PAGER"];

/// Whether `name`/`value` is a pager override that disables interactive paging,
/// e.g. `GIT_PAGER=cat` or an empty `PAGER`.
pub fn is_disabling_pager_override(name: &str, value: &str) -> bool {
    if !PAGER_OVERRIDE_ENV_VARS.contains(&name) {
        return false;
    }
    let value = value.trim();
    value.is_empty() || value.eq_ignore_ascii_case("cat") || value.eq_ignore_ascii_case("true")
}

/// Remove pager-disabling overrides from an environment map before using it to
/// spawn a terminal or serve as the login-shell environment.
pub fn remove_pager_disabling_overrides(env: &mut HashMap<String, String>) {
    env.retain(|name, value| !is_disabling_pager_override(name, value));
}

/// Drop pager-disabling overrides that were inherited from the launching
/// process out of this process, so spawns that never touch the shell-env cache
/// (e.g. a failed capture) stay clean as well.
fn scrub_pager_disabling_process_vars() {
    for name in PAGER_OVERRIDE_ENV_VARS {
        if let Ok(value) = std::env::var(name) {
            if is_disabling_pager_override(name, &value) {
                // SAFETY: called during app startup / background env refresh
                // before / while spawning user shells, same as
                // `apply_environment_map` below.
                unsafe { std::env::remove_var(name) };
            }
        }
    }
}

fn shell_env_cache_path() -> PathBuf {
    let base = if cfg!(target_os = "macos") {
        dirs::home_dir()
            .map(|home| home.join("Library/Application Support/Terry"))
            .unwrap_or_else(|| PathBuf::from("."))
    } else if cfg!(target_os = "windows") {
        dirs::data_local_dir()
            .map(|dir| dir.join("Terry"))
            .unwrap_or_else(|| PathBuf::from("."))
    } else {
        dirs::data_local_dir()
            .map(|dir| dir.join("terry"))
            .unwrap_or_else(|| PathBuf::from("."))
    };
    base.join("shell_env.json")
}

/// True after a login-shell capture has been applied (cache or live).
pub fn is_shell_env_loaded() -> bool {
    SHELL_ENV_LOADED.load(Ordering::Acquire)
}

fn mark_shell_env_loaded() {
    SHELL_ENV_LOADED.store(true, Ordering::Release);
    if let Ok(mut waiters) = SHELL_ENV_WAITERS.lock() {
        for tx in waiters.drain(..) {
            let _ = tx.send(());
        }
    }
}

/// Wait until the login-shell environment (cached or freshly captured) is ready.
pub async fn wait_until_shell_env_loaded() {
    if is_shell_env_loaded() {
        return;
    }
    let (tx, rx) = oneshot::channel();
    {
        let Ok(mut waiters) = SHELL_ENV_WAITERS.lock() else {
            return;
        };
        if is_shell_env_loaded() {
            return;
        }
        waiters.push(tx);
    }
    let _ = rx.await;
}

/// Apply the last successful login-shell environment from disk so terminals can
/// start before a fresh multi-second `zsh -lic` capture finishes.
pub fn apply_cached_environment() -> bool {
    let path = shell_env_cache_path();
    let Ok(bytes) = std::fs::read(&path) else {
        return false;
    };
    let Ok(env_map) = serde_json::from_slice::<HashMap<String, String>>(&bytes) else {
        return false;
    };
    if env_map.is_empty() {
        return false;
    }
    apply_environment_map(&env_map);
    mark_shell_env_loaded();
    log::info!(
        "applied cached shell environment from {} ({} vars)",
        path.display(),
        env_map.len()
    );
    true
}

fn save_environment_cache(env_map: &HashMap<String, String>) {
    let path = shell_env_cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Never persist pager overrides that disable paging: a single launch from a
    // host that injects them (e.g. an IDE setting `GIT_PAGER=cat`) would
    // otherwise stick in the cache and disable paging in every later session.
    let mut env_map = env_map.clone();
    remove_pager_disabling_overrides(&mut env_map);
    if let Ok(json) = serde_json::to_vec(&env_map) {
        let _ = std::fs::write(path, json);
    }
}

fn apply_environment_map(env_map: &HashMap<String, String>) {
    scrub_pager_disabling_process_vars();
    for (name, value) in env_map {
        // Skip SHLVL to prevent it from polluting the process environment.
        // The login shell used for env capture increments SHLVL, and if we
        // propagate it, terminals inherit it and increment again.
        if name == "SHLVL" {
            continue;
        }
        // Pager overrides are launcher junk when they disable paging; never
        // apply them, or `git log` would dump its output instead of paging.
        if is_disabling_pager_override(name, value) {
            continue;
        }
        // SAFETY: called during app startup / background env refresh before /
        // while spawning user shells. Same pattern as prior set_var usage.
        unsafe { std::env::set_var(name, value) };
    }
}

/// Apply a captured login-shell environment to this process and persist it for
/// the next cold start.
pub fn apply_environment_map_and_cache(env_map: &HashMap<String, String>) {
    apply_environment_map(env_map);
    save_environment_cache(env_map);
    mark_shell_env_loaded();
}

/// Unblock waiters even when login-shell capture fails, so terminals can still
/// start with the current process environment.
pub fn notify_shell_env_ready() {
    scrub_pager_disabling_process_vars();
    mark_shell_env_loaded();
}

/// Snapshot of the current process environment for terminal spawning.
pub fn process_environment_map() -> HashMap<String, String> {
    std::env::vars().collect()
}

fn parse_env_map_from_noisy_output(output: &str) -> Result<collections::HashMap<String, String>> {
    for (position, _) in output.match_indices('{') {
        let candidate = &output[position..];
        let mut deserializer = serde_json::Deserializer::from_str(candidate);
        if let Ok(env_map) = HashMap::<String, String>::deserialize(&mut deserializer) {
            return Ok(env_map);
        }
    }
    anyhow::bail!("Failed to find JSON in shell output: {output}")
}

pub fn print_env() {
    let env_vars: HashMap<String, String> = std::env::vars().collect();
    let json = serde_json::to_string_pretty(&env_vars).unwrap_or_else(|err| {
        eprintln!("Error serializing environment variables: {}", err);
        std::process::exit(1);
    });
    println!("{}", json);
}

/// Capture all environment variables from the login shell in the given directory.
pub async fn capture(
    shell_path: impl AsRef<Path>,
    args: &[String],
    directory: impl AsRef<Path>,
) -> Result<collections::HashMap<String, String>> {
    #[cfg(windows)]
    return capture_windows(shell_path.as_ref(), args, directory.as_ref()).await;
    #[cfg(unix)]
    return capture_unix(shell_path.as_ref(), args, directory.as_ref()).await;
}

/// Try to parse the environment output before checking the exit status.
/// The user's shell rc files may contain commands that fail (e.g. editor
/// integrations that call posix_spawnp outside a real PTY), causing a
/// non-zero exit status even though `zed --printenv` ran successfully and
/// produced valid output on its separate fd.
fn parse_env_output(
    env_output: &str,
    status: &std::process::ExitStatus,
    successful_capture_warning: impl FnOnce() -> String,
    failed_capture_error: impl FnOnce() -> String,
) -> Result<collections::HashMap<String, String>> {
    match parse_env_map_from_noisy_output(env_output) {
        Ok(env_map) => {
            if !status.success() {
                log::warn!("{}", successful_capture_warning());
            }
            Ok(env_map)
        }
        Err(parse_error) => {
            if !status.success() {
                anyhow::bail!(
                    "{}. Failed to deserialize environment variables from json: {parse_error}. output: {env_output}",
                    failed_capture_error(),
                );
            }

            anyhow::bail!(
                "Failed to deserialize environment variables from json: {parse_error}. output: {env_output}"
            );
        }
    }
}

#[cfg(unix)]
async fn capture_unix(
    shell_path: &Path,
    args: &[String],
    directory: &Path,
) -> Result<collections::HashMap<String, String>> {
    use std::os::unix::process::CommandExt;

    use crate::command::new_std_command;

    let shell_kind = ShellKind::new(shell_path, false);
    let quoted_zed_path = super::get_shell_safe_zed_path(shell_kind)?;

    let mut command_string = String::new();
    let mut command = new_std_command(shell_path);
    command.args(args);
    // Strip launcher-injected pager overrides (e.g. `GIT_PAGER=cat` set by an
    // IDE or agent shell) so the captured environment only reflects what the
    // user's dotfiles set up; otherwise they would be cached and disable
    // paging in every terminal spawned afterwards.
    for name in PAGER_OVERRIDE_ENV_VARS {
        command.env_remove(name);
    }
    // In some shells, file descriptors greater than 2 cannot be used in interactive mode,
    // so file descriptor 0 (stdin) is used instead. This impacts zsh, old bash; perhaps others.
    // See: https://github.com/zed-industries/zed/pull/32136#issuecomment-2999645482
    const FD_STDIN: std::os::fd::RawFd = 0;
    const FD_STDOUT: std::os::fd::RawFd = 1;
    const FD_STDERR: std::os::fd::RawFd = 2;

    let (fd_num, redir) = match shell_kind {
        ShellKind::Rc => (FD_STDIN, format!(">[1={}]", FD_STDIN)), // `[1=0]`
        ShellKind::Nushell | ShellKind::Tcsh => (FD_STDOUT, "".to_string()),
        // xonsh doesn't support redirecting to stdin, and control sequences are printed to
        // stdout on startup
        ShellKind::Xonsh => (FD_STDERR, "o>e".to_string()),
        ShellKind::PowerShell => (FD_STDIN, format!(">{}", FD_STDIN)),
        _ => (FD_STDIN, format!(">&{}", FD_STDIN)), // `>&0`
    };

    match shell_kind {
        ShellKind::Csh | ShellKind::Tcsh => {
            // For csh/tcsh, login shell requires passing `-` as 0th argument (instead of `-l`)
            command.arg0("-");
        }
        ShellKind::Fish => {
            // in fish, asdf, direnv attach to the `fish_prompt` event
            command_string.push_str("emit fish_prompt;");
            command.arg("-l");
        }
        _ => {
            command.arg("-l");
        }
    }

    match shell_kind {
        // Nushell does not allow non-interactive login shells.
        // Instead of doing "-l -i -c '<command>'"
        // use "-l -e '<command>; exit'" instead
        ShellKind::Nushell => command.arg("-e"),
        _ => command.args(["-i", "-c"]),
    };

    // Prefix with "./" if the path starts with "-" to prevent cd from interpreting it as a flag
    let dir_str = directory.to_string_lossy();
    let dir_str = if dir_str.starts_with('-') {
        format!("./{dir_str}").into()
    } else {
        dir_str
    };
    let quoted_dir = shell_kind
        .try_quote(&dir_str)
        .context("unexpected null in directory name")?;

    // cd into the directory, triggering directory specific side-effects (asdf, direnv, etc)
    command_string.push_str(&format!("cd {};", quoted_dir));
    if let Some(prefix) = shell_kind.command_prefix() {
        command_string.push(prefix);
    }
    command_string.push_str(&format!("{} --printenv {}", quoted_zed_path, redir));

    if let ShellKind::Nushell = shell_kind {
        command_string.push_str("; exit");
    }

    command.arg(&command_string);

    super::set_pre_exec_to_start_new_session(&mut command);

    let (env_output, process_output) = spawn_and_read_fd(command, fd_num).await?;
    let env_output = String::from_utf8_lossy(&env_output);

    parse_env_output(
        &env_output,
        &process_output.status,
        || {
            format!(
                "login shell exited with {} but environment was captured successfully. stderr: {:?}",
                process_output.status,
                String::from_utf8_lossy(&process_output.stderr),
            )
        },
        || {
            format!(
                "login shell exited with {}. stdout: {:?}, stderr: {:?}",
                process_output.status,
                String::from_utf8_lossy(&process_output.stdout),
                String::from_utf8_lossy(&process_output.stderr),
            )
        },
    )
}

#[cfg(unix)]
async fn spawn_and_read_fd(
    mut command: std::process::Command,
    child_fd: std::os::fd::RawFd,
) -> anyhow::Result<(Vec<u8>, std::process::Output)> {
    use command_fds::{CommandFdExt, FdMapping};
    use std::{io::Read, process::Stdio};

    let (mut reader, writer) = std::io::pipe()?;

    command.fd_mappings(vec![FdMapping {
        parent_fd: writer.into(),
        child_fd,
    }])?;

    let process = smol::process::Command::from(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer)?;

    Ok((buffer, process.output().await?))
}

#[cfg(windows)]
async fn capture_windows(
    shell_path: &Path,
    args: &[String],
    directory: &Path,
) -> Result<collections::HashMap<String, String>> {
    use std::process::Stdio;

    let zed_path =
        std::env::current_exe().context("Failed to determine current zed executable path.")?;

    let shell_kind = ShellKind::new(shell_path, true);
    // Prefix with "./" if the path starts with "-" to prevent cd from interpreting it as a flag
    let directory_string = directory.display().to_string();
    let directory_string = if directory_string.starts_with('-') {
        format!("./{directory_string}")
    } else {
        directory_string
    };
    let zed_path_string = zed_path.display().to_string();
    let quote_for_shell = |value: &str| {
        shell_kind
            .try_quote(value)
            .map(|quoted| quoted.into_owned())
            .context("unexpected null in directory name")
    };
    let mut cmd = crate::command::new_command(shell_path);
    cmd.args(args);
    // See `capture_unix`: never let launcher-injected pager overrides leak
    // into the captured login-shell environment.
    for name in PAGER_OVERRIDE_ENV_VARS {
        cmd.env_remove(name);
    }
    let quoted_directory = quote_for_shell(&directory_string)?;
    let quoted_zed_path = quote_for_shell(&zed_path_string)?;
    let cmd = match shell_kind {
        ShellKind::Csh
        | ShellKind::Tcsh
        | ShellKind::Rc
        | ShellKind::Fish
        | ShellKind::Xonsh
        | ShellKind::Posix => cmd.args([
            "-l",
            "-i",
            "-c",
            &format!("cd {}; {} --printenv", quoted_directory, quoted_zed_path),
        ]),
        ShellKind::PowerShell | ShellKind::Pwsh => cmd.args([
            "-NonInteractive",
            "-NoProfile",
            "-Command",
            &format!(
                "Set-Location {}; & {} --printenv",
                quoted_directory, quoted_zed_path
            ),
        ]),
        ShellKind::Elvish => cmd.args([
            "-c",
            &format!("cd {}; {} --printenv", quoted_directory, quoted_zed_path),
        ]),
        ShellKind::Nushell => {
            let zed_command = shell_kind
                .prepend_command_prefix(&quoted_zed_path)
                .into_owned();
            cmd.args([
                "-c",
                &format!("cd {}; {} --printenv", quoted_directory, zed_command),
            ])
        }
        ShellKind::Cmd => {
            let dir = directory_string.trim_end_matches('\\');
            cmd.args(["/d", "/c", "cd", dir, "&&", &zed_path_string, "--printenv"])
        }
    }
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let output = cmd
        .output()
        .await
        .with_context(|| format!("command {cmd:?}"))?;
    let env_output = String::from_utf8_lossy(&output.stdout);

    parse_env_output(
        &env_output,
        &output.status,
        || {
            format!(
                "Command {cmd:?} exited with {} but environment was captured successfully. stderr: {:?}",
                output.status,
                String::from_utf8_lossy(&output.stderr),
            )
        },
        || {
            format!(
                "Command {cmd:?} failed with {}. stdout: {:?}, stderr: {:?}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use std::process::ExitStatus;

    use super::*;
    use crate::path;

    #[cfg(unix)]
    fn exit_status(code: i32) -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;

        ExitStatus::from_raw(code << 8)
    }

    #[cfg(windows)]
    fn exit_status(code: u32) -> ExitStatus {
        use std::os::windows::process::ExitStatusExt;

        ExitStatus::from_raw(code)
    }

    #[test]
    fn parse_env_output_accepts_valid_env_when_shell_exits_nonzero() {
        let env_json = serde_json::json!({
            "PATH": path!("/usr/bin"),
            "SHELL": path!("/bin/zsh"),
        });
        let env_output = format!("shell startup noise\n{env_json}\nshell shutdown noise");

        let env_map = parse_env_output(
            &env_output,
            &exit_status(1),
            || "shell exited with 1 but environment was captured successfully".to_string(),
            || panic!("failed capture error should not be evaluated for valid environment output"),
        )
        .expect("valid environment output should be returned despite non-zero shell exit");
        assert_eq!(
            env_map.get("PATH").map(String::as_str),
            Some(path!("/usr/bin"))
        );
        assert_eq!(
            env_map.get("SHELL").map(String::as_str),
            Some(path!("/bin/zsh"))
        );
    }

    #[test]
    fn disabling_pager_overrides_are_detected() {
        assert!(is_disabling_pager_override("GIT_PAGER", "cat"));
        assert!(is_disabling_pager_override("GIT_PAGER", "CAT"));
        assert!(is_disabling_pager_override("PAGER", ""));
        assert!(is_disabling_pager_override("PAGER", " true "));
        assert!(!is_disabling_pager_override("GIT_PAGER", "less"));
        assert!(!is_disabling_pager_override("GIT_PAGER", "delta -n"));
        // Variables outside the pager override list are never touched.
        assert!(!is_disabling_pager_override("LESS", "cat"));
        assert!(!is_disabling_pager_override("GIT_EDITOR", "cat"));
    }

    #[test]
    fn remove_pager_disabling_overrides_strips_only_disabling_values() {
        let mut env = HashMap::default();
        env.insert("GIT_PAGER".to_string(), "cat".to_string());
        env.insert("PAGER".to_string(), "less".to_string());
        env.insert("PATH".to_string(), path!("/usr/bin").to_string());

        remove_pager_disabling_overrides(&mut env);

        assert_eq!(env.get("GIT_PAGER"), None);
        assert_eq!(env.get("PAGER").map(String::as_str), Some("less"));
        assert_eq!(env.get("PATH").map(String::as_str), Some(path!("/usr/bin")));
    }
}
