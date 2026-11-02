//! What coding agents exist, how to launch them, and how to recognise them.
//!
//! This is the catalogue behind "start a coding agent here": the list of CLIs
//! Terry knows about, the one-shot commands for continuing or forking a session,
//! and the process/title matching that tells which agent is already running.
//!
//! It lives in its own crate with no dependencies at all because two very
//! different hosts need it:
//!
//! - the GUI, which draws an agent badge in the sidebar and offers a "resume
//!   this session" action;
//! - the terminal daemon, which has no window but must still let a phone start
//!   an agent and know what it is running.
//!
//! They must agree on both the command lines and the matching rules, so the data
//! and the logic live here rather than being duplicated.

/// One coding-agent CLI.
///
/// Commands are only set when the CLI documents a one-shot flag or subcommand.
/// Agents without a published command stay detectable, so a live process can
/// still be reattached, but they do not get a guessed flag.
#[derive(Clone, Copy, Debug)]
pub struct AgentCli {
    /// Stable identifier, used by integrations and persisted sessions.
    pub id: &'static str,
    /// Executable names this agent may appear as.
    pub names: &'static [&'static str],
    /// Executable to launch when Terry starts it itself.
    pub bin: &'static str,
    /// One-shot command continuing the most recent session.
    pub resume: Option<&'static str>,
    /// One-shot command letting the user pick a past session.
    pub past: Option<&'static str>,
    /// One-shot command forking the most recent session.
    pub fork: Option<&'static str>,
}

/// Every agent Terry can recognise or launch.
pub fn coding_agents() -> &'static [AgentCli] {
    &[
        AgentCli {
            id: "claude",
            names: &["claude", "claude-code"],
            bin: "claude",
            resume: Some("{bin} --continue"),
            past: Some("{bin} --resume"),
            fork: Some("{bin} --continue --fork-session"),
        },
        AgentCli {
            id: "codex",
            names: &["codex"],
            bin: "codex",
            resume: Some("{bin} resume --last"),
            past: Some("{bin} resume"),
            fork: Some("{bin} fork"),
        },
        AgentCli {
            id: "trae",
            names: &["traecli", "trae", "traecode"],
            bin: "traecli",
            resume: Some("{bin} resume"),
            past: None,
            fork: Some("{bin} fork"),
        },
        AgentCli {
            id: "opencode",
            names: &["opencode"],
            bin: "opencode",
            resume: Some("{bin} --continue"),
            past: None,
            fork: Some("{bin} --continue --fork"),
        },
        AgentCli {
            id: "omp",
            names: &["omp", "oh-my-pi"],
            bin: "omp",
            resume: Some("{bin} --continue"),
            past: Some("{bin} --resume"),
            // `--fork` requires a session id, so there is no one-shot command.
            fork: None,
        },
        AgentCli {
            id: "qoder",
            names: &["qoder", "qodercli"],
            bin: "qoder",
            resume: Some("{bin} -c"),
            past: Some("{bin} -r"),
            fork: Some("{bin} --fork-session -r"),
        },
        AgentCli {
            id: "qoder-cn",
            names: &["qoder-cn"],
            bin: "qoder-cn",
            resume: Some("{bin} -c"),
            past: Some("{bin} -r"),
            fork: Some("{bin} --fork-session -r"),
        },
        AgentCli {
            id: "gemini",
            names: &["gemini"],
            bin: "gemini",
            resume: Some("{bin} --resume"),
            past: None,
            fork: None,
        },
        AgentCli {
            id: "cursor",
            names: &["cursor-agent", "cursor"],
            bin: "cursor-agent",
            resume: Some("{bin} --resume"),
            past: None,
            fork: None,
        },
        // Detected, and a still-running process is reattached. No documented
        // one-shot resume, fork, or past-session command.
        AgentCli {
            id: "grok",
            names: &["grok"],
            bin: "grok",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "prime",
            names: &["prime-agent", "prime"],
            bin: "prime-agent",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "droid",
            names: &["droid"],
            bin: "droid",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "qwen",
            names: &["qwen", "qwen-code"],
            bin: "qwen",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "goose",
            names: &["goose"],
            bin: "goose",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "codebuddy",
            names: &["codebuddy"],
            bin: "codebuddy",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "copilot",
            names: &["copilot"],
            bin: "copilot",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "kimi",
            names: &["kimi"],
            bin: "kimi",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "pi",
            names: &["pi"],
            bin: "pi",
            resume: None,
            past: None,
            fork: None,
        },
        // Terry's built-in coding agent. It is the `tom` subcommand of Terry's
        // own binary, so the shim on PATH is what runs. `-c` continues the most
        // recent session for the working directory, which is what a reopened
        // tab wants.
        AgentCli {
            id: "tom",
            names: &["tom"],
            bin: "tom",
            resume: Some("{bin} -c"),
            past: Some("{bin} -r"),
            fork: None,
        },
        AgentCli {
            id: "crush",
            names: &["crush"],
            bin: "crush",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "antigravity",
            names: &["agy", "antigravity"],
            bin: "agy",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "aider",
            names: &["aider"],
            bin: "aider",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "amp",
            names: &["amp"],
            bin: "amp",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "auggie",
            names: &["auggie"],
            bin: "auggie",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "hermes",
            names: &["hermes"],
            bin: "hermes",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "vibe",
            names: &["vibe"],
            bin: "vibe",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "empryo",
            names: &["empryo"],
            bin: "empryo",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "cline",
            names: &["cline"],
            bin: "cline",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "devin",
            names: &["devin", "devin-cli"],
            bin: "devin",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "mastracode",
            names: &["mastracode", "mastra-code"],
            bin: "mastracode",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "kiro",
            names: &["kiro", "kiro-cli"],
            bin: "kiro-cli",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "kilo",
            names: &["kilo", "kilo-code"],
            bin: "kilo",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "letta",
            names: &["letta", "letta-code"],
            bin: "letta",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "maki",
            names: &["maki"],
            bin: "maki",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "muse",
            names: &["muse", "muse-code", "muse-cli"],
            bin: "muse",
            resume: None,
            past: None,
            fork: None,
        },
    ]
}

/// The agent with this id, if the catalogue knows it.
pub fn agent_by_id(id: &str) -> Option<&'static AgentCli> {
    coding_agents().iter().find(|agent| agent.id == id)
}

/// The basename of a possibly-qualified executable path, lowercased.
///
/// Windows paths and a trailing `.exe` are handled because the same helper runs
/// on both platforms.
pub fn command_basename(name: &str) -> String {
    let base = name
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or(name);
    let base = base
        .strip_suffix(".exe")
        .or_else(|| base.strip_suffix(".EXE"))
        .unwrap_or(base);
    base.to_ascii_lowercase()
}

/// The agent whose executable name matches `base` (already lowercased).
///
/// Versioned shims such as `claude-2` count as the same agent.
pub fn agent_for_basename(base: &str) -> Option<&'static AgentCli> {
    coding_agents().iter().find(|agent| {
        agent.names.iter().any(|name| {
            base == *name
                || base.starts_with(&format!("{name}-"))
                || base.starts_with(&format!("{name}_"))
        })
    })
}

/// The executable name to use for this agent when it was invoked as `base`.
///
/// An agent can be installed under an alias (`claude-code`), so the invocation
/// name is preferred over the canonical one for building a command line.
pub fn invocation_bin(agent: &AgentCli, base: &str) -> &'static str {
    agent
        .names
        .iter()
        .copied()
        .find(|name| *name == base)
        .unwrap_or(agent.bin)
}

/// Recognises coding agents installed as npm packages.
///
/// npm-installed CLIs run as `node .../node_modules/<package>/dist/cli.js`, so
/// the foreground process name is the runtime (`node`) rather than the agent.
/// Matching the package entry point keeps those terminals identifiable.
pub fn agent_for_package_path(path: &str) -> Option<&'static AgentCli> {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();

    const PACKAGES: &[(&str, &str)] = &[
        (
            "node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js",
            "pi",
        ),
        (
            "node_modules/@earendil-works/pi-coding-agent/dist/cli.js",
            "pi",
        ),
        ("node_modules/@oh-my-pi/pi-coding-agent/dist/cli.js", "omp"),
        ("node_modules/@moonshot-ai/kimi-code/dist/main.mjs", "kimi"),
        ("node_modules/@qwen-code/qwen-code/dist/index.js", "qwen"),
        ("node_modules/@qwen-code/qwen-code/dist/index.mjs", "qwen"),
        ("node_modules/mastracode/dist/cli", "mastracode"),
        ("node_modules/mastracode/dist/cli.js", "mastracode"),
        ("node_modules/@letta-ai/letta-code/letta", "letta"),
    ];

    for (suffix, id) in PACKAGES {
        if normalized.ends_with(suffix) {
            return agent_by_id(id);
        }
    }
    None
}

/// Whether `name` is a coding agent's executable.
pub fn is_coding_agent_name(name: &str) -> bool {
    agent_for_basename(&command_basename(name)).is_some()
}

/// An agent mentioned in free text (a title or breadcrumb), and the executable
/// name it appeared under.
///
/// Short command names (`pi`, `amp`, `omp`, `agy`) are excluded: matching them
/// inside a title produces false positives, so they are only recognised from a
/// process name.
pub fn agent_mentioned_in_text(text: &str) -> Option<(&'static AgentCli, &'static str)> {
    text.split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
        .filter(|token| token.chars().count() >= 4)
        .find_map(|token| {
            let base = command_basename(token);
            let agent = agent_for_basename(&base)?;
            Some((agent, invocation_bin(agent, &base)))
        })
}

/// Whether `text` mentions a coding agent.
pub fn text_mentions_coding_agent(text: &str) -> bool {
    agent_mentioned_in_text(text).is_some()
}

/// Substitutes the executable name into a command template.
///
/// `{bin}` is the only placeholder these templates use, and it is always
/// replaced with an allowlisted name from [`AgentCli::bin`] or
/// [`AgentCli::names`], never with caller-supplied text.
pub fn agent_command(template: &str, bin: &str) -> String {
    template.replace("{bin}", bin)
}

/// Quotes a value for POSIX shells.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// One-shot command that resumes a specific session, as reported by an
/// integration.
///
/// Only agents whose CLI documents a session-id flag appear here; anything else
/// returns `None` rather than guessing.
pub fn session_resume_command(agent_id: &str, session: &str) -> Option<String> {
    let template = match agent_id {
        "pi" => "{bin} --session {session}",
        "opencode" => "{bin} --session {session}",
        "claude" => "{bin} --resume {session}",
        "kimi" => "{bin} --session {session}",
        "qwen" => "{bin} --resume {session}",
        "codex" => "{bin} resume {session}",
        // tom resumes a specific session by id.
        "tom" => "{bin} -r {session}",
        _ => return None,
    };
    let agent = agent_by_id(agent_id)?;
    Some(
        template
            .replace("{bin}", agent.bin)
            .replace("{session}", &shell_quote(session)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for agent in coding_agents() {
            assert!(seen.insert(agent.id), "duplicate agent id {:?}", agent.id);
        }
    }

    #[test]
    fn every_agent_can_be_found_by_its_own_id() {
        for agent in coding_agents() {
            let found = agent_by_id(agent.id).expect(agent.id);
            assert_eq!(found.id, agent.id);
        }
    }

    #[test]
    fn names_and_binaries_are_lowercase() {
        // Matching is done against a lowercased basename, so an uppercase entry
        // would silently never match.
        for agent in coding_agents() {
            assert_eq!(agent.bin, agent.bin.to_ascii_lowercase(), "{}", agent.id);
            for name in agent.names {
                assert_eq!(*name, name.to_ascii_lowercase(), "{}", agent.id);
            }
        }
    }

    #[test]
    fn basename_handles_paths_and_windows_suffixes() {
        assert_eq!(command_basename("/usr/local/bin/claude"), "claude");
        assert_eq!(command_basename("C:\\Tools\\Codex.EXE"), "codex");
        assert_eq!(command_basename("claude"), "claude");
    }

    #[test]
    fn versioned_shims_match_their_agent() {
        let agent = agent_for_basename("claude-2").expect("versioned shim");
        assert_eq!(agent.id, "claude");
        let agent = agent_for_basename("kimi_code").expect("underscore shim");
        assert_eq!(agent.id, "kimi");
    }

    #[test]
    fn an_alias_prefers_the_name_it_was_invoked_as() {
        let agent = agent_by_id("claude").expect("claude");
        assert_eq!(invocation_bin(agent, "claude-code"), "claude-code");
        // An unknown basename falls back to the canonical executable.
        assert_eq!(invocation_bin(agent, "something-else"), "claude");
    }

    #[test]
    fn npm_hosted_agents_are_recognised_by_package_path() {
        let agent = agent_for_package_path(
            "/usr/lib/node_modules/@qwen-code/qwen-code/dist/index.js",
        )
        .expect("qwen installed with npm runs as node");
        assert_eq!(agent.id, "qwen");
    }

    #[test]
    fn short_names_are_not_matched_inside_titles() {
        // `pi` and `amp` are real agents, but they appear inside ordinary words,
        // so title matching must not see them.
        assert!(agent_mentioned_in_text("compare the api responses").is_none());
        // A long, distinctive name is fine.
        let (agent, bin) = agent_mentioned_in_text("claude — myproject").expect("claude");
        assert_eq!(agent.id, "claude");
        assert_eq!(bin, "claude");
    }

    #[test]
    fn launching_uses_the_documented_flags_only() {
        // Agents without a published command must not get a guessed flag: an
        // invented `--continue` would make the terminal exit with a usage error.
        for agent in coding_agents() {
            for template in [agent.resume, agent.past, agent.fork].into_iter().flatten() {
                assert!(
                    template.contains("{bin}"),
                    "{} template would not run the agent: {template}",
                    agent.id
                );
            }
        }
    }

    #[test]
    fn resuming_a_reported_session_quotes_the_id() {
        let command = session_resume_command("claude", "abc; rm -rf /").expect("claude");
        assert_eq!(command, "claude --resume 'abc; rm -rf /'");
        // An agent without a documented session flag must not guess.
        assert!(session_resume_command("aider", "abc").is_none());
    }

    #[test]
    fn shell_quoting_escapes_single_quotes() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
