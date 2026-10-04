//! Terry shell integration: automatic setup for zsh and fish, plus a manually
//! sourced script for bash.
//!
//! When a terminal is spawned, Terry writes its integration scripts to the app
//! data dir and injects environment variables so supported shells load them.
//! The scripts emit OSC 133 (prompt marks) and OSC 7 (working directory)
//! sequences, which power features like `JumpToPreviousPrompt`.

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use anyhow::Context as _;
use collections::HashMap;

use paths::data_dir;

const TERRY_INTEGRATION_DIR_ENV: &str = "TERRY_INTEGRATION_DIR";
const TERRY_USER_ZDOTDIR_ENV: &str = "TERRY_USER_ZDOTDIR";

const ZSHENV_SHIM: &str = include_str!("../../../assets/shell-integration/zsh/.zshenv");
const ZSHRC_SHIM: &str = include_str!("../../../assets/shell-integration/zsh/.zshrc");
const TERRY_ZSH: &str = include_str!("../../../assets/shell-integration/zsh/terry.zsh");
const FISH_INTEGRATION: &str =
    include_str!("../../../assets/shell-integration/fish/vendor_conf.d/terry_integration.fish");
const TERRY_BASH: &str = include_str!("../../../assets/shell-integration/bash/terry.bash");

const SCRIPTS: &[(&str, &str)] = &[
    ("zsh/.zshenv", ZSHENV_SHIM),
    ("zsh/.zshrc", ZSHRC_SHIM),
    ("zsh/terry.zsh", TERRY_ZSH),
    (
        "fish/vendor_conf.d/terry_integration.fish",
        FISH_INTEGRATION,
    ),
    ("bash/terry.bash", TERRY_BASH),
];

static INTEGRATION_DIR: OnceLock<PathBuf> = OnceLock::new();

/// The directory the integration scripts are written to, resolved once per
/// launch.
fn integration_dir() -> &'static PathBuf {
    INTEGRATION_DIR.get_or_init(|| data_dir().join("shell-integration"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SupportedShell {
    Zsh,
    Fish,
    Bash,
}

fn detect_shell(program: &str) -> Option<SupportedShell> {
    // Ignore a leading `-` (login shells are often launched as `-zsh`).
    let name = program
        .rsplit(['/', '\\'])
        .next()?
        .trim_start_matches('-')
        .to_lowercase();
    match name.as_str() {
        "zsh" => Some(SupportedShell::Zsh),
        "fish" => Some(SupportedShell::Fish),
        "bash" => Some(SupportedShell::Bash),
        _ => None,
    }
}

/// Writes the integration scripts to `dir`, skipping files that are already up
/// to date.
fn write_scripts(dir: &Path) -> anyhow::Result<()> {
    for (relative_path, contents) in SCRIPTS {
        let path = dir.join(relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        if std::fs::read_to_string(&path).is_ok_and(|existing| existing == *contents) {
            continue;
        }
        std::fs::write(&path, contents)
            .with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(())
}

/// Sets up Terry's shell integration for the shell launched as `program` by
/// injecting the required environment variables into `env`. Does nothing for
/// unsupported shells (or remote shells like `ssh`) and when integration was
/// already injected for this environment, as happens when
/// `TerminalBuilder::clone_builder` re-runs `TerminalBuilder::new` with an
/// already-prepared template.
pub(crate) fn inject_shell_integration_env(
    env: &mut HashMap<String, String>,
    program: &str,
) -> anyhow::Result<()> {
    if env.contains_key(TERRY_INTEGRATION_DIR_ENV) {
        return Ok(());
    }
    let dir = integration_dir();
    write_scripts(dir)?;
    inject_with_dir(env, program, dir);
    Ok(())
}

fn inject_with_dir(env: &mut HashMap<String, String>, program: &str, dir: &Path) {
    if env.contains_key(TERRY_INTEGRATION_DIR_ENV) {
        // Already injected: never overwrite the saved user ZDOTDIR with the
        // shim directory, or re-running `new` on a cloned builder would make
        // the shim self-reference.
        return;
    }
    let Some(shell) = detect_shell(program) else {
        return;
    };
    env.insert("TERRY_SHELL_INTEGRATION".to_string(), "1".to_string());
    env.insert(
        TERRY_INTEGRATION_DIR_ENV.to_string(),
        dir.display().to_string(),
    );
    match shell {
        SupportedShell::Zsh => {
            // The .zshenv shim restores this value before anything else runs,
            // so the user's own startup files are loaded from their real
            // location.
            if let Some(user_zdotdir) = env.get("ZDOTDIR") {
                env.insert(TERRY_USER_ZDOTDIR_ENV.to_string(), user_zdotdir.clone());
            }
            env.insert("ZDOTDIR".to_string(), dir.join("zsh").display().to_string());
        }
        SupportedShell::Fish => {
            // Fish loads `vendor_conf.d` scripts from `XDG_DATA_DIRS`; prepend
            // our directory so the integration is picked up automatically.
            let data_dirs = match env.get("XDG_DATA_DIRS") {
                Some(existing) if !existing.trim().is_empty() => {
                    format!("{}:{}", dir.display(), existing)
                }
                _ => format!("{}:/usr/local/share:/usr/share", dir.display()),
            };
            env.insert("XDG_DATA_DIRS".to_string(), data_dirs);
        }
        SupportedShell::Bash => {
            // bash has no automatic injection point; the user sources the
            // script manually via $TERRY_INTEGRATION_DIR.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_shells_are_skipped() {
        for program in ["/usr/bin/nu", "pwsh.exe", "ssh", "/usr/bin/ssh", "sh"] {
            let mut env = HashMap::default();
            inject_with_dir(&mut env, program, Path::new("/integration"));
            assert!(env.is_empty(), "{program} should not be injected");
        }
    }

    #[test]
    fn login_shell_prefix_and_case_are_ignored() {
        let mut env = HashMap::default();
        inject_with_dir(&mut env, "-ZSH", Path::new("/integration"));
        assert_eq!(
            env.get("ZDOTDIR").map(String::as_str),
            Some("/integration/zsh")
        );
    }

    #[test]
    fn zsh_env_preserves_user_zdotdir() {
        let mut env = HashMap::default();
        env.insert("ZDOTDIR".to_string(), "/home/user/.config/zsh".to_string());
        inject_with_dir(&mut env, "/bin/zsh", Path::new("/integration"));
        assert_eq!(
            env.get(TERRY_USER_ZDOTDIR_ENV).map(String::as_str),
            Some("/home/user/.config/zsh")
        );
        assert_eq!(
            env.get("ZDOTDIR").map(String::as_str),
            Some("/integration/zsh")
        );
        assert_eq!(
            env.get(TERRY_INTEGRATION_DIR_ENV).map(String::as_str),
            Some("/integration")
        );
    }

    #[test]
    fn zsh_env_without_zdotdir_does_not_set_user_zdotdir() {
        let mut env = HashMap::default();
        inject_with_dir(&mut env, "zsh", Path::new("/integration"));
        assert!(!env.contains_key(TERRY_USER_ZDOTDIR_ENV));
        assert_eq!(
            env.get("ZDOTDIR").map(String::as_str),
            Some("/integration/zsh")
        );
    }

    #[test]
    fn fish_prepends_to_xdg_data_dirs() {
        let mut env = HashMap::default();
        env.insert(
            "XDG_DATA_DIRS".to_string(),
            "/opt/homebrew/share".to_string(),
        );
        inject_with_dir(
            &mut env,
            "/opt/homebrew/bin/fish",
            Path::new("/integration"),
        );
        assert_eq!(
            env.get("XDG_DATA_DIRS").map(String::as_str),
            Some("/integration:/opt/homebrew/share")
        );

        // Unset or empty XDG_DATA_DIRS falls back to fish's defaults.
        let mut env = HashMap::default();
        inject_with_dir(&mut env, "fish", Path::new("/integration"));
        assert_eq!(
            env.get("XDG_DATA_DIRS").map(String::as_str),
            Some("/integration:/usr/local/share:/usr/share")
        );

        let mut env = HashMap::default();
        env.insert("XDG_DATA_DIRS".to_string(), "  ".to_string());
        inject_with_dir(&mut env, "fish", Path::new("/integration"));
        assert_eq!(
            env.get("XDG_DATA_DIRS").map(String::as_str),
            Some("/integration:/usr/local/share:/usr/share")
        );
    }

    #[test]
    fn bash_only_gets_the_integration_dir() {
        let mut env = HashMap::default();
        env.insert("ZDOTDIR".to_string(), "/keep/out".to_string());
        inject_with_dir(&mut env, "/bin/bash", Path::new("/integration"));
        assert_eq!(
            env.get(TERRY_INTEGRATION_DIR_ENV).map(String::as_str),
            Some("/integration")
        );
        assert_eq!(env.get("ZDOTDIR").map(String::as_str), Some("/keep/out"));
        assert!(!env.contains_key("XDG_DATA_DIRS"));
    }

    #[test]
    fn injection_is_idempotent() {
        let mut env = HashMap::default();
        env.insert("ZDOTDIR".to_string(), "/user/zsh".to_string());
        inject_with_dir(&mut env, "zsh", Path::new("/integration"));
        let injected = env.clone();
        // Re-running the injection (as `clone_builder` does) must not clobber
        // the saved user ZDOTDIR with the shim directory.
        inject_with_dir(&mut env, "zsh", Path::new("/integration"));
        assert_eq!(env, injected);
        assert_eq!(
            env.get(TERRY_USER_ZDOTDIR_ENV).map(String::as_str),
            Some("/user/zsh")
        );
    }

    #[test]
    fn write_scripts_writes_all_scripts() {
        let dir = std::env::temp_dir().join(format!(
            "terry-shell-integration-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        write_scripts(&dir).unwrap();
        for (relative_path, contents) in SCRIPTS {
            let path = dir.join(relative_path);
            let written = std::fs::read_to_string(&path).unwrap();
            assert_eq!(written, *contents, "{} should match", path.display());
        }
        // Writing again is a no-op and must not fail.
        write_scripts(&dir).unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }
}
