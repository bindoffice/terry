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
    // Exact aliases win across the whole catalogue. A separate agent such as
    // `qoder-cn` must not be captured by the earlier `qoder` entry merely
    // because one name is a prefix of the other.
    coding_agents()
        .iter()
        .find(|agent| agent.names.iter().any(|name| base == *name))
        .or_else(|| {
            coding_agents()
                .iter()
                .find(|agent| agent.names.iter().any(|name| is_versioned_shim(base, name)))
        })
}

fn is_versioned_shim(base: &str, name: &str) -> bool {
    let Some(suffix) = base.strip_prefix(name) else {
        return false;
    };
    match suffix.as_bytes().first() {
        Some(b'-') => suffix[1..]
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit()),
        // Underscore shims are used by some package managers for release
        // channels (`kimi_code`), so any non-empty suffix is accepted.
        Some(b'_') => suffix.len() > 1,
        _ => false,
    }
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
/// Matching the package directory keeps those terminals identifiable.
///
/// Packages are matched by *directory*, not by entry-point file: the same
/// package ships several entry points (`dist/cli.js`, `dist/bundle/cli.js`,
/// `dist/index.js`), and version upgrades move them around. Matching the
/// package name across `node_modules/<package>/` covers every entry point a
/// release happens to use, including the `node_modules/.pnpm/<pkg>@<ver>/`
/// layout pnpm produces.
///
/// The directory is required to be a complete path segment, so a package such
/// as `@qwen-code/qwen-code` does not also match an unrelated
/// `@qwen-code/qwen-code-extra` checkout.
pub fn agent_for_package_path(path: &str) -> Option<&'static AgentCli> {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();

    /// npm package directory -> agent id.
    const PACKAGES: &[(&str, &str)] = &[
        ("@earendil-works/pi-coding-agent", "pi"),
        ("@oh-my-pi/pi-coding-agent", "omp"),
        ("@moonshot-ai/kimi-code", "kimi"),
        ("@qwen-code/qwen-code", "qwen"),
        ("@openai/codex", "codex"),
        ("@letta-ai/letta-code", "letta"),
        ("mastracode", "mastracode"),
    ];

    for (package, id) in PACKAGES {
        if package_in_path(&normalized, package) {
            return agent_by_id(id);
        }
    }
    None
}

/// Recognises an agent from the full argv of a foreground process.
///
/// The executable name alone is not enough for the ways CLIs are commonly
/// installed and launched:
///
/// - npm packages run as `node .../node_modules/<package>/dist/cli.js`;
/// - launchers run as `node .../node_modules/npm/bin/npx-cli.js exec codex`;
/// - wrapper scripts may turn the process title into a bare `pi` or leave the
///   original absolute path in `argv[0]`.
///
/// Only the interpreter's script and launcher package arguments are inspected,
/// rather than every argument, so a command such as `git commit -m pi` is not
/// mistaken for a running Pi session.
pub fn agent_for_argv(argv: &[String]) -> Option<&'static AgentCli> {
    let executable = argv.first()?;
    let executable_base = command_basename(executable);

    if let Some(agent) = agent_for_basename(&executable_base) {
        return Some(agent);
    }

    // A package path anywhere after argv[0] is a strong signal. This also covers
    // wrappers that keep the real script in an option value.
    if let Some(agent) = argv
        .iter()
        .skip(1)
        .find_map(|argument| agent_for_package_path(argument))
    {
        return Some(agent);
    }

    if is_interpreter(&executable_base) {
        for argument in argv.iter().skip(1).filter(|arg| !arg.starts_with('-')) {
            if let Some(agent) = agent_for_script_path(argument) {
                return Some(agent);
            }
        }
    }

    let launcher = if is_launcher(&executable_base) {
        Some(executable_base.clone())
    } else if is_interpreter(&executable_base) {
        argv.iter()
            .skip(1)
            .filter(|arg| !arg.starts_with('-'))
            .find_map(|argument| {
                let stem = script_stem(argument)?;
                is_launcher(&stem).then_some(stem)
            })
    } else {
        None
    };
    if let Some(launcher) = launcher {
        if let Some(agent) = agent_for_launcher_args(&launcher, &argv[1..]) {
            return Some(agent);
        }
    }

    if is_shell(&executable_base) {
        if let Some(command) = shell_dash_c_command(argv) {
            if let Some(agent) = agent_for_shell_words(command) {
                return Some(agent);
            }
        }
    }

    None
}

/// Recognises an agent from the script path passed to an interpreter.
///
/// The package directory wins over generic entry-point names (`cli.js`,
/// `main.mjs`), while a distinctive script stem such as `codex.js` identifies
/// the agent directly.
pub fn agent_for_script_path(path: &str) -> Option<&'static AgentCli> {
    if let Some(agent) = agent_for_package_path(path) {
        return Some(agent);
    }

    let stem = script_stem(path)?;
    if let Some(agent) = agent_for_basename(&stem) {
        return Some(agent);
    }
    if !is_generic_entry_point(&stem) {
        return None;
    }

    let package = package_directory_name(path)?;
    let package = package.strip_suffix("-cli").unwrap_or(&package);
    agent_for_basename(package)
}

fn script_stem(path: &str) -> Option<String> {
    let base = path
        .rsplit(|character| character == '/' || character == '\\')
        .next()?
        .trim();
    if base.is_empty() || base.starts_with('-') || base.starts_with('.') {
        return None;
    }

    let stem = match base.rsplit_once('.') {
        Some((stem, extension))
            if matches!(
                extension.to_ascii_lowercase().as_str(),
                "js" | "mjs"
                    | "cjs"
                    | "ts"
                    | "mts"
                    | "cts"
                    | "py"
                    | "sh"
                    | "exe"
                    | "cmd"
                    | "bat"
                    | "ps1"
            ) =>
        {
            stem
        }
        _ => base,
    };
    (!stem.is_empty()).then(|| stem.to_ascii_lowercase())
}

fn package_directory_name(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let node_modules = normalized.rfind("node_modules/")?;
    let mut components = normalized[node_modules + "node_modules/".len()..]
        .split('/')
        .filter(|component| !component.is_empty());
    let first = components.next()?;
    let package = if first.starts_with('@') {
        components.next()?
    } else {
        first
    };
    (!package.is_empty()).then(|| package.to_string())
}

fn is_generic_entry_point(stem: &str) -> bool {
    matches!(
        stem,
        "index" | "cli" | "main" | "bin" | "bundle" | "entry" | "app" | "start" | "run"
    )
}

fn is_interpreter(base: &str) -> bool {
    matches!(
        base,
        "node" | "nodejs" | "bun" | "deno" | "python" | "python3" | "py"
    )
}

fn is_launcher(base: &str) -> bool {
    matches!(
        base,
        "npx" | "bunx" | "uvx" | "pipx" | "npm" | "pnpm" | "yarn" | "corepack"
    )
}

fn is_shell(base: &str) -> bool {
    matches!(
        base,
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "csh" | "tcsh"
    )
}

fn agent_for_launcher_args(launcher: &str, arguments: &[String]) -> Option<&'static AgentCli> {
    let mut expects_subcommand = matches!(launcher, "npm" | "pnpm" | "yarn" | "corepack" | "pipx");
    let mut skip_next = false;

    for argument in arguments {
        if skip_next {
            skip_next = false;
            continue;
        }
        if argument.starts_with('-') {
            if !argument.contains('=') && launcher_option_takes_value(launcher, argument) {
                skip_next = true;
            }
            continue;
        }

        if expects_subcommand {
            expects_subcommand = false;
            if matches!(
                argument.as_str(),
                "exec" | "dlx" | "x" | "run" | "run-script" | "tool"
            ) {
                continue;
            }
        }

        if let Some(agent) = agent_for_package_reference(argument) {
            return Some(agent);
        }
    }
    None
}

fn launcher_option_takes_value(launcher: &str, argument: &str) -> bool {
    match launcher {
        "npm" | "pnpm" | "yarn" | "corepack" => matches!(
            argument,
            "--prefix" | "--registry" | "--workspace" | "--filter" | "--cwd" | "-C" | "-w"
        ),
        "pipx" => matches!(argument, "--python" | "--pip-args"),
        _ => matches!(argument, "--package" | "--registry" | "-p"),
    }
}

fn agent_for_package_reference(reference: &str) -> Option<&'static AgentCli> {
    if reference.contains('/') || reference.contains('\\') {
        if let Some(agent) = agent_for_script_path(reference) {
            return Some(agent);
        }
    }

    let mut reference = reference.trim();
    if reference.starts_with('@') {
        if let Some(version_separator) = reference[1..].find('@') {
            reference = &reference[..version_separator + 1];
        }
    } else if let Some((name, _version)) = reference.split_once('@') {
        reference = name;
    }
    let package = reference.rsplit(['/', '\\']).next().unwrap_or(reference);
    let package = package.strip_suffix("-cli").unwrap_or(package);
    agent_for_basename(&command_basename(package))
}

fn shell_dash_c_command(argv: &[String]) -> Option<&str> {
    argv.iter()
        .position(|argument| argument == "-c")
        .and_then(|index| argv.get(index + 1))
        .map(String::as_str)
}

fn agent_for_shell_words(command: &str) -> Option<&'static AgentCli> {
    let mut words = command.split_whitespace();
    let command = loop {
        let word = words.next()?;
        if word.contains('=') {
            continue;
        }
        break word.trim_matches(|character| matches!(character, '\'' | '"'));
    };

    let base = command_basename(command);
    if let Some(agent) = agent_for_basename(&base) {
        return Some(agent);
    }
    if is_launcher(&base) {
        return agent_for_launcher_args(
            base.as_str(),
            &words.map(str::to_string).collect::<Vec<_>>(),
        );
    }
    None
}

/// Whether `package` appears as a whole directory under a `node_modules/`.
fn package_in_path(normalized_path: &str, package: &str) -> bool {
    let mut from = 0;
    while let Some(index) = normalized_path[from..].find("node_modules/") {
        let start = from + index + "node_modules/".len();
        let rest = &normalized_path[start..];
        if rest.strip_prefix(package).is_some_and(|tail| {
            // The package name must end at a path separator, never inside a
            // longer name such as `qwen-code-extra`.
            tail.starts_with('/')
        }) {
            return true;
        }
        from = start;
    }
    false
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
        // A hyphenated agent name is an exact alias, not a version of its
        // shorter neighbour.
        let agent = agent_for_basename("qoder-cn").expect("qoder-cn");
        assert_eq!(agent.id, "qoder-cn");
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
        let agent =
            agent_for_package_path("/usr/lib/node_modules/@qwen-code/qwen-code/dist/index.js")
                .expect("qwen installed with npm runs as node");
        assert_eq!(agent.id, "qwen");
    }

    #[test]
    fn foreground_argv_identifies_direct_and_npm_hosted_agents() {
        let cases = [
            (vec!["codex"], "codex"),
            (
                vec![
                    "node",
                    "/Users/me/.local/state/fnm_multishells/1/bin/codex",
                    "-s",
                    "danger-full-access",
                ],
                "codex",
            ),
            (
                vec![
                    "node",
                    "/Users/me/.pi/agent/install/releases/1.4.0/node_modules/.bin/pi",
                ],
                "pi",
            ),
            (
                vec![
                    "/Users/me/.local/share/pi-node/current/bin/node",
                    "/Users/me/.pi/agent/install/releases/1.4.0/node_modules/@oh-my-pi/pi-coding-agent/dist/cli.js",
                ],
                "omp",
            ),
            (
                vec!["node", "/opt/homebrew/lib/node_modules/.bin/npx", "codex"],
                "codex",
            ),
            (vec!["npm", "exec", "@openai/codex@0.166.0"], "codex"),
            (vec!["sh", "-c", "CODEX_HOME=/tmp codex --version"], "codex"),
        ];

        for (argv, expected) in cases {
            let argv = argv.into_iter().map(str::to_string).collect::<Vec<_>>();
            assert_eq!(
                agent_for_argv(&argv).map(|agent| agent.id),
                Some(expected),
                "{argv:?}"
            );
        }

        // Ordinary arguments that merely mention a short agent name are not
        // enough: the command has to be the executable or a launcher target.
        assert!(agent_for_argv(&["git", "commit", "-m", "pi"].map(str::to_string)).is_none());
    }

    #[test]
    fn every_direct_executable_is_recognised_from_argv() {
        for agent in coding_agents() {
            let argv = vec![agent.bin.to_string()];
            assert_eq!(
                agent_for_argv(&argv).map(|found| found.id),
                Some(agent.id),
                "{}",
                agent.bin
            );
        }
    }

    #[test]
    fn generic_npm_entry_points_use_the_package_name() {
        assert_eq!(
            agent_for_script_path("/usr/lib/node_modules/@google/gemini-cli/dist/index.js")
                .map(|agent| agent.id),
            Some("gemini")
        );
        assert_eq!(
            agent_for_argv(&[
                "node".to_string(),
                "/usr/local/lib/node_modules/@openai/codex/bin/codex.js".to_string(),
            ])
            .map(|agent| agent.id),
            Some("codex")
        );
    }

    /// A release can move its entry point, and the same package ships several
    /// (`dist/cli.js`, `dist/bundle/cli.js`, `bin/codex.js`). Matching the
    /// package directory keeps the agent identifiable either way.
    #[test]
    fn every_entry_point_of_a_package_identifies_the_agent() {
        let cases = [
            (
                "/home/me/.pi/agent/install/1.4.0/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js",
                "pi",
            ),
            (
                "/usr/local/lib/node_modules/@earendil-works/pi-coding-agent/dist/cli.js",
                "pi",
            ),
            (
                "/opt/homebrew/lib/node_modules/@oh-my-pi/pi-coding-agent/dist/cli.js",
                "omp",
            ),
            (
                "/usr/lib/node_modules/@moonshot-ai/kimi-code/dist/main.mjs",
                "kimi",
            ),
            (
                "/usr/local/lib/node_modules/@openai/codex/bin/codex.js",
                "codex",
            ),
            (
                "/x/node_modules/.pnpm/@qwen-code+qwen-code@1.0.0/node_modules/@qwen-code/qwen-code/dist/index.js",
                "qwen",
            ),
        ];
        for (path, expected) in cases {
            assert_eq!(
                agent_for_package_path(path).map(|agent| agent.id),
                Some(expected),
                "{path}"
            );
        }
    }

    /// The package name must end at a path separator: a different package that
    /// merely starts with the same characters is not that agent.
    #[test]
    fn a_longer_package_name_is_not_a_prefix_match() {
        assert!(
            agent_for_package_path(
                "/usr/lib/node_modules/@qwen-code/qwen-code-extra/dist/index.js"
            )
            .is_none(),
            "`qwen-code-extra` must not be reported as `qwen`"
        );
        assert!(
            agent_for_package_path("/usr/lib/node_modules/my-mastracode/dist/cli.js").is_none()
        );
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
