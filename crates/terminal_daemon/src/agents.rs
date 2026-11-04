//! Agent state for sessions owned by the daemon.
//!
//! The daemon owns the PTY master and can inspect its foreground process group
//! to identify a running agent. State comes from these sources, in this order:
//!
//! 1. **What the agent reported.** The agent integration scripts
//!    (`assets/agent-integrations/*`) POST `/api/report_agent_state` with the
//!    per-terminal token the daemon injected into their environment. This is
//!    authoritative and also covers agents the rule table cannot read.
//! 2. **What the screen looks like.** For a foreground-identified agent,
//!    `agent_detect` matches the bottom lines
//!    and the title against a per-agent rule table.
//!
//! Identity comes from a report when available, otherwise from the foreground
//! process. If neither exists, the session is not treated as an agent session.

use std::collections::HashMap;
use std::sync::Mutex;

use agent_detect::{AgentState, Screen};

/// A state an agent integration reported for a session.
#[derive(Clone, Debug)]
pub struct Reported {
    pub agent: Option<String>,
    pub state: AgentState,
    pub message: Option<String>,
}

/// Per-session agent bookkeeping, shared by the control and HTTP handlers.
#[derive(Default)]
pub struct AgentRegistry {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Keyed by the terminal token the integration was launched with: that is
    /// what the scripts send, and what the daemon injected into their env.
    reported: HashMap<String, Reported>,
    /// Session id -> the terminal token it was spawned with.
    session_tokens: HashMap<String, String>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records which terminal token belongs to a session.
    pub fn bind_session(&self, session: &str, token: &str) {
        self.lock()
            .session_tokens
            .insert(session.to_string(), token.to_string());
    }

    /// Drops everything known about a session.
    pub fn forget_session(&self, session: &str) {
        let mut inner = self.lock();
        if let Some(token) = inner.session_tokens.remove(session) {
            inner.reported.remove(&token);
        }
    }

    /// Applies a report from an in-terminal integration.
    ///
    /// The token has to belong to a session this daemon spawned; otherwise
    /// anything that can reach the port could paint fake agent states.
    pub fn report(
        &self,
        token: &str,
        agent: Option<String>,
        state_word: &str,
        message: Option<String>,
    ) -> Result<(), String> {
        let state = match state_word {
            "working" => AgentState::Working,
            "blocked" => AgentState::Blocked,
            "idle" => AgentState::Idle,
            other => return Err(format!("unknown agent state {other:?}")),
        };

        let mut inner = self.lock();
        if !inner.session_tokens.values().any(|known| known == token) {
            return Err("unknown terminal token".to_string());
        }
        let entry = inner.reported.entry(token.to_string()).or_insert(Reported {
            agent: None,
            state,
            message: None,
        });
        if agent.is_some() {
            entry.agent = agent;
        }
        entry.state = state;
        entry.message = message;
        Ok(())
    }

    /// Records the agent running in a session, without touching its state.
    ///
    /// `report_agent_session` fires at session start, before the agent has done
    /// anything; treating that as `idle` would overwrite a `working` report that
    /// arrived first.
    pub fn set_agent(&self, token: &str, agent: &str) -> Result<(), String> {
        let mut inner = self.lock();
        if !inner.session_tokens.values().any(|known| known == token) {
            return Err("unknown terminal token".to_string());
        }
        let entry = inner.reported.entry(token.to_string()).or_insert(Reported {
            agent: None,
            state: AgentState::Idle,
            message: None,
        });
        entry.agent = Some(agent.to_string());
        Ok(())
    }

    /// The session a terminal token was bound to.
    pub fn session_for_token(&self, token: &str) -> Option<String> {
        let inner = self.lock();
        inner
            .session_tokens
            .iter()
            .find(|(_, known)| known.as_str() == token)
            .map(|(session, _)| session.clone())
    }

    /// The report for a session, if its integration ever sent one.
    pub fn reported_for(&self, session: &str) -> Option<Reported> {
        let inner = self.lock();
        let token = inner.session_tokens.get(session)?;
        inner.reported.get(token).cloned()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // The map holds plain data only; a panic while updating it must not take
        // the daemon (and every session it owns) down with it.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// What a session's agent state is, and which agent it is.
pub struct AgentStatus {
    pub agent: Option<String>,
    pub state: AgentState,
    /// True when the state came from a report rather than the screen.
    pub reported: bool,
}

/// Resolves a session's agent state.
///
/// A report wins when there is one. Otherwise `agent` comes from the daemon's
/// foreground-process inspection and the screen is matched against that agent's
/// rules only.
///
/// That second argument is not optional bookkeeping. Rules are written to be
/// tried against the agent they belong to: codex's rule table, for instance,
/// matches a plain shell prompt and reports `Idle`. Probing every agent's rules
/// would therefore turn ordinary terminals into idle agents. When an agent has
/// no matching screen rule, `active` provides a coarse working/idle fallback so
/// process-identified agents still stay visible.
pub fn resolve(
    reported: Option<&Reported>,
    agent: Option<&str>,
    title: &str,
    lines: &[String],
    active: bool,
) -> Option<AgentStatus> {
    if let Some(reported) = reported {
        return Some(AgentStatus {
            agent: reported.agent.clone().or_else(|| agent.map(str::to_string)),
            state: reported.state,
            reported: true,
        });
    }

    let agent = agent?;
    if agent_catalog::agent_by_id(agent).is_none() {
        return None;
    }
    let screen = Screen {
        lines: lines.to_vec(),
        title: title.to_string(),
    };
    // Foreground-process identity is authoritative. If the agent has no screen
    // rules (or none match), keep it listed and use recent output as a coarse
    // working/idle fallback instead of treating it as an ordinary shell.
    let state = agent_detect::detect(agent, &screen).unwrap_or(if active {
        AgentState::Working
    } else {
        AgentState::Idle
    });
    Some(AgentStatus {
        agent: Some(agent.to_string()),
        state,
        reported: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_from_a_known_session_is_recorded() {
        let registry = AgentRegistry::new();
        registry.bind_session("t-1", "tok-1");

        registry
            .report("tok-1", Some("claude".into()), "blocked", None)
            .expect("report");

        let reported = registry.reported_for("t-1").expect("recorded");
        assert_eq!(reported.state, AgentState::Blocked);
        assert_eq!(reported.agent.as_deref(), Some("claude"));
    }

    #[test]
    fn a_report_with_an_unknown_token_is_rejected() {
        let registry = AgentRegistry::new();
        registry.bind_session("t-1", "tok-1");

        let error = registry
            .report("tok-other", None, "working", None)
            .expect_err("unknown tokens must not be able to paint state");
        assert!(error.contains("unknown terminal token"));
    }

    #[test]
    fn an_unknown_state_word_is_rejected() {
        let registry = AgentRegistry::new();
        registry.bind_session("t-1", "tok-1");

        assert!(
            registry.report("tok-1", None, "busy", None).is_err(),
            "only working/blocked/idle are valid"
        );
    }

    #[test]
    fn forgetting_a_session_drops_its_report() {
        let registry = AgentRegistry::new();
        registry.bind_session("t-1", "tok-1");
        registry
            .report("tok-1", None, "idle", None)
            .expect("report");

        registry.forget_session("t-1");
        assert!(registry.reported_for("t-1").is_none());
    }

    #[test]
    fn a_report_takes_precedence_over_the_screen() {
        let reported = Reported {
            agent: Some("claude".into()),
            state: AgentState::Blocked,
            message: None,
        };

        // The screen says nothing recognisable, but the report is authoritative.
        let status = resolve(Some(&reported), None, "", &[], false).expect("resolved");
        assert_eq!(status.state, AgentState::Blocked);
        assert!(status.reported);
    }

    #[test]
    fn the_screen_is_used_when_the_agent_is_known() {
        // A claude permission prompt, matching the real rule shape: the footer
        // hint and the numbered choice have to be there together.
        let lines = vec![
            "Do you want to proceed?".to_string(),
            "❯ 1. yes".to_string(),
            "  2. no".to_string(),
            "Esc to cancel".to_string(),
        ];

        let status =
            resolve(None, Some("claude"), "claude", &lines, false).expect("a rule should match");
        assert!(!status.reported);
        assert_eq!(
            status.state,
            AgentState::Blocked,
            "agent: {:?}",
            status.agent
        );
    }

    #[test]
    fn a_session_with_no_known_agent_is_not_listed() {
        // This is the important negative case. Codex's rule table matches a
        // plain shell prompt and reports `Idle`, so probing every agent would
        // turn ordinary terminals into idle agents. Without a known agent the
        // answer must be "not an agent terminal".
        let lines = vec!["$ ls".to_string(), "src  Cargo.toml".to_string()];

        assert!(
            resolve(None, None, "zsh", &lines, false).is_none(),
            "an unknown session must stay unknown, not be probed"
        );
        assert!(
            resolve(None, Some("zsh"), "zsh", &lines, false).is_none(),
            "a non-agent program has no rules to match"
        );
    }

    #[test]
    fn the_known_agent_alone_decides_which_rules_run() {
        // Same screen, known agent: codex reads it as idle.
        let lines = vec!["$ ls".to_string()];
        let as_codex = resolve(None, Some("codex"), "codex", &lines, false);
        assert_eq!(
            as_codex.map(|status| status.state),
            Some(AgentState::Idle),
            "codex rules do match this screen; that is exactly why the agent \
             must come from a report rather than from probing"
        );
    }

    #[test]
    fn a_process_identified_agent_stays_listed_without_screen_rules() {
        // Aider is in the process catalogue but has no agent_detect rule set.
        // Foreground-process identity must still make the terminal visible.
        let status = resolve(None, Some("aider"), "aider", &[], true).expect("resolved");
        assert_eq!(status.agent.as_deref(), Some("aider"));
        assert_eq!(status.state, AgentState::Working);
    }
}
