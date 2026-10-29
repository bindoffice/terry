//! Installable agent integrations.
//!
//! These are small hooks written into an agent's own extension directory. They
//! report lifecycle state to Terry's IPC so the sidebar can show `blocked`
//! exactly, instead of inferring it from the screen.

use std::path::PathBuf;

const PI_EXTENSION: &str = include_str!("../assets/agent-integrations/pi/terry-agent-state.ts");
const PI_EXTENSION_NAME: &str = "terry-agent-state.ts";
const OPENCODE_PLUGIN: &str =
    include_str!("../assets/agent-integrations/opencode/terry-agent-state.js");
const OPENCODE_PLUGIN_NAME: &str = "terry-agent-state.js";
const CLAUDE_SCRIPT: &str =
    include_str!("../assets/agent-integrations/claude/terry-agent-state.sh");
const CLAUDE_SCRIPT_NAME: &str = "terry-agent-state.sh";
const KIMI_SCRIPT: &str = include_str!("../assets/agent-integrations/kimi/terry-agent-state.sh");
const KIMI_SCRIPT_NAME: &str = "terry-agent-state.sh";
const QWEN_SCRIPT: &str = include_str!("../assets/agent-integrations/qwen/terry-agent-session.sh");
const QWEN_SCRIPT_NAME: &str = "terry-agent-session.sh";
const CODEX_SCRIPT: &str =
    include_str!("../assets/agent-integrations/codex/terry-agent-session.sh");
const CODEX_SCRIPT_NAME: &str = "terry-agent-session.sh";

/// Agents with an installable integration.
pub const SUPPORTED: &[&str] = &["pi", "opencode", "claude", "kimi", "qwen", "codex"];

/// Kimi Code hooks: event, optional matcher, and the action passed to the
/// script. Ported from herdr's integration table.
const KIMI_EVENTS: &[(&str, Option<&str>, &str)] = &[
    ("SessionStart", None, "session"),
    ("UserPromptSubmit", None, "working"),
    ("PreToolUse", Some("^(?!AskUserQuestion$).*$"), "working"),
    ("PreToolUse", Some("^AskUserQuestion$"), "blocked"),
    ("PostToolUse", Some("^AskUserQuestion$"), "working"),
    ("PostToolUseFailure", Some("^AskUserQuestion$"), "working"),
    ("SubagentStart", None, "working"),
    ("PreCompact", None, "working"),
    ("PermissionRequest", None, "blocked"),
    ("PermissionResult", None, "working"),
    ("Stop", None, "idle"),
    ("Interrupt", None, "idle"),
];

/// Directory Pi loads extensions from (`$PI_CODING_AGENT_DIR` or `~/.pi/agent`).
pub fn pi_extension_dir() -> anyhow::Result<PathBuf> {
    if let Ok(dir) = std::env::var("PI_CODING_AGENT_DIR") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir).join("extensions"));
        }
    }
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home)
        .join(".pi")
        .join("agent")
        .join("extensions"))
}

/// Directory OpenCode loads global plugins from
/// (`$OPENCODE_CONFIG_DIR` or `~/.config/opencode`).
pub fn opencode_plugin_dir() -> anyhow::Result<PathBuf> {
    if let Ok(dir) = std::env::var("OPENCODE_CONFIG_DIR") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir).join("plugin"));
        }
    }
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("opencode")
        .join("plugin"))
}

/// Path of the Claude Code hook script
/// (`$CLAUDE_CONFIG_DIR` or `~/.claude`).
pub fn claude_script_path() -> anyhow::Result<PathBuf> {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir).join(CLAUDE_SCRIPT_NAME));
        }
    }
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".claude").join(CLAUDE_SCRIPT_NAME))
}

/// The `hooks` block to merge into `~/.claude/settings.json`.
///
/// Terry does not rewrite the file itself: Claude's settings schema is
/// user-owned and merging blindly risks dropping unrelated hooks.
pub fn claude_settings_snippet(script: &std::path::Path) -> String {
    let script = script.display().to_string();
    let command = |event: &str| format!("{script} {event}");
    let hook = |event: &str, matcher: bool| {
        let mut entry = serde_json::json!({
            "hooks": [{ "type": "command", "command": command(event) }]
        });
        if matcher {
            entry["matcher"] = serde_json::json!("*");
        }
        serde_json::json!([entry])
    };

    let snippet = serde_json::json!({
        "hooks": {
            "SessionStart": hook("SessionStart", false),
            "UserPromptSubmit": hook("UserPromptSubmit", false),
            "PreToolUse": hook("PreToolUse", true),
            "PostToolUse": hook("PostToolUse", true),
            "Stop": hook("Stop", false),
            "Notification": hook("Notification", false),
            "SessionEnd": hook("SessionEnd", false),
        }
    });
    serde_json::to_string_pretty(&snippet).unwrap_or_default()
}

/// Directory Kimi Code reads hooks from (`$KIMI_CODE_HOME` or `~/.kimi-code`).
pub fn kimi_hook_dir() -> anyhow::Result<PathBuf> {
    if let Ok(dir) = std::env::var("KIMI_CODE_HOME") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir).join("hooks"));
        }
    }
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".kimi-code").join("hooks"))
}

/// Directory Qwen Code reads hooks from (`$QWEN_CONFIG_DIR` or `~/.qwen`).
pub fn qwen_hook_dir() -> anyhow::Result<PathBuf> {
    if let Ok(dir) = std::env::var("QWEN_CONFIG_DIR") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir).join("hooks"));
        }
    }
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".qwen").join("hooks"))
}

/// `[[hooks]]` tables to append to `~/.kimi-code/config.toml`.
pub fn kimi_settings_snippet(script: &std::path::Path) -> String {
    let script = script.display().to_string();
    let mut out = String::new();
    for (event, matcher, action) in KIMI_EVENTS {
        out.push_str("[[hooks]]\n");
        out.push_str(&format!("event = \"{event}\"\n"));
        if let Some(matcher) = matcher {
            out.push_str(&format!("matcher = \"{matcher}\"\n"));
        }
        out.push_str(&format!("command = \"{script} {action}\"\n"));
        out.push_str("timeout = 10\n\n");
    }
    out
}

/// `hooks` block to merge into `~/.qwen/settings.json`.
pub fn qwen_settings_snippet(script: &std::path::Path) -> String {
    let script = script.display().to_string();
    let snippet = serde_json::json!({
        "hooks": {
            "SessionStart": [{
                "matcher": "*",
                "hooks": [{
                    "type": "command",
                    "command": format!("{script} session"),
                    "timeout": 10000
                }]
            }]
        }
    });
    serde_json::to_string_pretty(&snippet).unwrap_or_default()
}

/// Directory Codex reads hooks from (`$CODEX_HOME` or `~/.codex`).
pub fn codex_dir() -> anyhow::Result<PathBuf> {
    if let Ok(dir) = std::env::var("CODEX_HOME") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".codex"))
}

/// `hooks.json` content to merge into `~/.codex/hooks.json`.
pub fn codex_hooks_snippet(script: &std::path::Path) -> String {
    let script = script.display().to_string();
    let snippet = serde_json::json!({
        "hooks": {
            "SessionStart": [{
                "hooks": [{
                    "type": "command",
                    "command": format!("{script} session"),
                    "timeout": 10
                }]
            }]
        }
    });
    serde_json::to_string_pretty(&snippet).unwrap_or_default()
}

/// `config.toml` addition that enables Codex hooks.
pub fn codex_config_snippet() -> String {
    "[features]\nhooks = true\n".to_string()
}

/// Installs the integration for `agent`, returning the written path.
pub fn install(agent: &str) -> anyhow::Result<PathBuf> {
    match agent {
        "pi" => install_pi(),
        "opencode" => install_file(
            opencode_plugin_dir()?,
            OPENCODE_PLUGIN_NAME,
            OPENCODE_PLUGIN,
        ),
        "claude" => install_claude(),
        "kimi" => install_executable(kimi_hook_dir()?, KIMI_SCRIPT_NAME, KIMI_SCRIPT),
        "qwen" => install_executable(qwen_hook_dir()?, QWEN_SCRIPT_NAME, QWEN_SCRIPT),
        "codex" => install_executable(codex_dir()?, CODEX_SCRIPT_NAME, CODEX_SCRIPT),
        other => anyhow::bail!(
            "unsupported agent integration {other:?}; supported: {}",
            SUPPORTED.join(", ")
        ),
    }
}

/// Removes the integration for `agent`, returning its path.
pub fn uninstall(agent: &str) -> anyhow::Result<PathBuf> {
    let path = match agent {
        "pi" => pi_extension_dir()?.join(PI_EXTENSION_NAME),
        "opencode" => opencode_plugin_dir()?.join(OPENCODE_PLUGIN_NAME),
        "claude" => claude_script_path()?,
        "kimi" => kimi_hook_dir()?.join(KIMI_SCRIPT_NAME),
        "qwen" => qwen_hook_dir()?.join(QWEN_SCRIPT_NAME),
        "codex" => codex_dir()?.join(CODEX_SCRIPT_NAME),
        other => anyhow::bail!(
            "unsupported agent integration {other:?}; supported: {}",
            SUPPORTED.join(", ")
        ),
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(path),
        Err(error) => Err(error.into()),
    }
}

fn install_pi() -> anyhow::Result<PathBuf> {
    install_file(pi_extension_dir()?, PI_EXTENSION_NAME, PI_EXTENSION)
}

fn install_claude() -> anyhow::Result<PathBuf> {
    let path = claude_script_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, CLAUDE_SCRIPT)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(path)
}

fn install_file(dir: PathBuf, name: &str, contents: &str) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    std::fs::write(&path, contents)?;
    Ok(path)
}

fn install_executable(dir: PathBuf, name: &str, contents: &str) -> anyhow::Result<PathBuf> {
    let path = install_file(dir, name, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_extension_is_embedded() {
        assert!(PI_EXTENSION.contains("TERRY_INTEGRATION_ID=pi"));
        assert!(PI_EXTENSION.contains("/api/report_agent_state"));
        assert!(PI_EXTENSION.contains("/api/report_agent_session"));
        assert!(PI_EXTENSION.contains("ui_prompt_start"));
    }

    #[test]
    fn opencode_plugin_is_embedded() {
        assert!(OPENCODE_PLUGIN.contains("TERRY_INTEGRATION_ID=opencode"));
        assert!(OPENCODE_PLUGIN.contains("/api/report_agent_state"));
        assert!(OPENCODE_PLUGIN.contains("/api/report_agent_session"));
        assert!(OPENCODE_PLUGIN.contains("permission.asked"));
        assert!(OPENCODE_PLUGIN.contains("session.idle"));
    }

    #[test]
    fn claude_hook_is_embedded() {
        assert!(CLAUDE_SCRIPT.contains("TERRY_INTEGRATION_ID=claude"));
        assert!(CLAUDE_SCRIPT.contains("/api/report_agent_state"));
        assert!(CLAUDE_SCRIPT.contains("/api/report_agent_session"));
        assert!(CLAUDE_SCRIPT.contains("SessionStart"));
        assert!(CLAUDE_SCRIPT.contains("Notification"));
    }

    #[test]
    fn claude_snippet_covers_the_lifecycle_events() {
        let snippet = claude_settings_snippet(std::path::Path::new("/tmp/hook.sh"));
        for event in [
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "Stop",
            "Notification",
            "SessionEnd",
        ] {
            assert!(snippet.contains(event), "missing {event} in {snippet}");
        }
        assert!(snippet.contains("/tmp/hook.sh SessionStart"));
    }

    #[test]
    fn kimi_and_qwen_snippets_cover_their_events() {
        let kimi = kimi_settings_snippet(std::path::Path::new("/tmp/hook.sh"));
        assert!(kimi.contains("event = \"SessionStart\""));
        assert!(kimi.contains("event = \"PermissionRequest\""));
        assert!(kimi.contains("command = \"/tmp/hook.sh blocked\""));
        assert!(kimi.contains("matcher = \"^AskUserQuestion$\""));

        let qwen = qwen_settings_snippet(std::path::Path::new("/tmp/hook.sh"));
        assert!(qwen.contains("SessionStart"));
        assert!(qwen.contains("/tmp/hook.sh session"));
    }

    #[test]
    fn codex_snippets_enable_hooks_and_report_the_session() {
        let hooks = codex_hooks_snippet(std::path::Path::new("/tmp/hook.sh"));
        assert!(hooks.contains("SessionStart"));
        assert!(hooks.contains("/tmp/hook.sh session"));
        assert!(codex_config_snippet().contains("hooks = true"));
    }

    #[test]
    fn unsupported_agents_are_rejected() {
        assert!(install("goose").is_err());
        assert!(uninstall("goose").is_err());
    }
}
