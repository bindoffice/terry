//! Lightweight update checker that polls the GitHub Releases API for the
//! latest Terry release and surfaces it through a status bar item and the
//! "Check for Updates" menu action.

use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use futures::AsyncReadExt;
use gpui::{actions, App, Context, Empty, EventEmitter, Global, Subscription, Window};
use http_client::{AsyncBody, HttpClient};
use release_channel::AppVersion;
use semver::Version;
use serde::Deserialize;
use ui::{Button, ButtonStyle, Icon, IconName, LabelSize, Tooltip, prelude::*};
use workspace::{HideStatusItem, StatusItemView, item::ItemHandle};

/// GitHub repository that publishes Terry releases.
const GITHUB_REPO: &str = "bindoffice/terry";

/// GitHub Releases API endpoint returning the newest non-draft, non-prerelease.
fn latest_release_url() -> String {
    format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest")
}

/// How long transient statuses (up-to-date / failed) stay visible in the
/// status bar before reverting to hidden.
const TRANSIENT_STATUS_DURATION: Duration = Duration::from_secs(4);

actions!(update_checker, [CheckForUpdates]);

/// A release published on GitHub.
#[derive(Clone, Debug)]
pub struct UpdateInfo {
    /// Newest version available on GitHub.
    pub version: Version,
    /// URL of the GitHub release page.
    pub url: String,
    /// Release title.
    pub title: String,
    /// ISO-8601 publish timestamp.
    pub published_at: String,
}

/// Where the update check currently stands.
#[derive(Clone, Debug)]
pub enum UpdateStatus {
    /// No check has run yet (or a transient status expired).
    Idle,
    /// A check is currently in flight.
    Checking,
    /// The running build is the newest release.
    UpToDate,
    /// A newer version is available.
    UpdateAvailable(UpdateInfo),
    /// The last check failed; the string is the error message.
    Failed(String),
}

/// Process-wide update checker state shared by the menu action, the status
/// bar item and the background check task.
pub struct UpdateCheckerState {
    status: UpdateStatus,
    /// Deadline until which a transient status remains visible.
    transient_until: Option<Instant>,
}

impl UpdateCheckerState {
    fn set_status(&mut self, status: UpdateStatus) {
        let transient = matches!(status, UpdateStatus::UpToDate | UpdateStatus::Failed(_));
        self.status = status;
        self.transient_until = transient
            .then(|| Instant::now() + TRANSIENT_STATUS_DURATION);
    }

    /// Clears a transient status once its display window has passed.
    fn expire_transient(&mut self) {
        if self
            .transient_until
            .is_some_and(|until| Instant::now() >= until)
        {
            self.status = UpdateStatus::Idle;
            self.transient_until = None;
        }
    }
}

impl Default for UpdateCheckerState {
    fn default() -> Self {
        Self {
            status: UpdateStatus::Idle,
            transient_until: None,
        }
    }
}

impl Global for UpdateCheckerState {}

/// Parses a GitHub release tag (with or without a leading `v`) as semver.
pub fn parse_github_version(tag: &str) -> Option<Version> {
    Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()
}

/// Parses the body of a GitHub "latest release" API response.
pub fn parse_latest_release_response(body: &str) -> Result<UpdateInfo> {
    let release: GitHubRelease = serde_json::from_str(body)?;
    let version = parse_github_version(&release.tag_name)
        .ok_or_else(|| anyhow!("unparseable release tag: {:?}", release.tag_name))?;
    Ok(UpdateInfo {
        version,
        url: release.html_url,
        title: release.name,
        published_at: release.published_at,
    })
}

/// Registers the global state and the "Check for Updates" action.
pub fn init(cx: &mut App) {
    cx.set_global(UpdateCheckerState::default());
    cx.on_action(|_: &CheckForUpdates, cx| check_for_updates(cx));
}

/// Starts an update check; the result lands in [`UpdateCheckerState`].
pub fn check_for_updates(cx: &mut App) {
    let already_checking = cx.read_global::<UpdateCheckerState, _>(|state, _| {
        matches!(state.status, UpdateStatus::Checking)
    });
    if already_checking {
        return;
    }
    cx.update_global::<UpdateCheckerState, _>(|state, _| state.set_status(UpdateStatus::Checking));

    let http_client = cx.http_client();
    let current_version = AppVersion::global(cx);
    let background_executor = cx.background_executor().clone();
    let cx = cx.to_async();
    cx.spawn(async move |cx| {
        let result = fetch_latest_release(http_client.as_ref()).await;
        let status = match result {
            Ok(release) if release.version > current_version => {
                UpdateStatus::UpdateAvailable(release)
            }
            Ok(_) => UpdateStatus::UpToDate,
            Err(error) => UpdateStatus::Failed(format!("{error:#}")),
        };
        let transient = matches!(status, UpdateStatus::UpToDate | UpdateStatus::Failed(_));
        cx.update(|cx| {
            cx.update_global::<UpdateCheckerState, _>(|state, _| state.set_status(status));
        });

        // Transient statuses (up-to-date / failed) disappear after a while.
        if transient {
            background_executor.timer(TRANSIENT_STATUS_DURATION).await;
            cx.update(|cx| {
                cx.update_global::<UpdateCheckerState, _>(|state, _| state.expire_transient());
            });
        }
    })
    .detach();
}

/// Status bar item showing the outcome of the latest update check.
pub struct UpdateStatusItem {
    _global_observation: Subscription,
}

impl UpdateStatusItem {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let _global_observation = cx.observe_global::<UpdateCheckerState>(|_, cx| cx.notify());
        Self { _global_observation }
    }
}

impl Render for UpdateStatusItem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = cx.global::<UpdateCheckerState>().status.clone();

        let element: gpui::AnyElement = match status {
            UpdateStatus::Idle => Empty.into_any_element(),
            UpdateStatus::Checking => Button::new(
                "update-checking",
                i18n::t("checking_for_updates"),
            )
            .label_size(LabelSize::Small)
            .loading(true)
            .into_any_element(),
            UpdateStatus::UpdateAvailable(release) => {
                let url = release.url.clone();
                Button::new(
                    "update-available",
                    format!("v{}", release.version),
                )
                .style(ButtonStyle::Filled)
                .label_size(LabelSize::Small)
                .start_icon(Icon::new(IconName::CloudDownload))
                .tooltip(Tooltip::text(format!(
                    "{} {}",
                    i18n::t("update_available"),
                    release.title
                )))
                .on_click(move |_, _window, cx| {
                    cx.open_url(&url);
                })
                .into_any_element()
            }
            UpdateStatus::UpToDate => Button::new("update-up-to-date", i18n::t("up_to_date"))
                .label_size(LabelSize::Small)
                .into_any_element(),
            UpdateStatus::Failed(message) => {
                Button::new("update-check-failed", i18n::t("update_check_failed"))
                    .label_size(LabelSize::Small)
                    .tooltip(Tooltip::text(message))
                    .on_click(|_, _window, cx| check_for_updates(cx))
                    .into_any_element()
            }
        };

        div().child(element)
    }
}

impl EventEmitter<workspace::ToolbarItemEvent> for UpdateStatusItem {}

impl StatusItemView for UpdateStatusItem {
    fn set_active_pane_item(
        &mut self,
        _: Option<&dyn ItemHandle>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        None
    }
}

/// GitHub Releases API response payload (only the fields we use).
#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    published_at: String,
}

async fn fetch_latest_release(http_client: &dyn HttpClient) -> Result<UpdateInfo> {
    let mut response = http_client
        .get(&latest_release_url(), AsyncBody::default(), true)
        .await
        .context("failed to reach the GitHub Releases API")?;
    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("GitHub Releases API returned {status}"));
    }
    let mut body = String::new();
    response
        .body_mut()
        .read_to_string(&mut body)
        .await
        .context("failed to read the release response")?;
    parse_latest_release_response(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_version_tags() {
        assert_eq!(parse_github_version("v1.2.3").unwrap(), Version::new(1, 2, 3));
        assert_eq!(parse_github_version("1.2.3").unwrap(), Version::new(1, 2, 3));
        assert_eq!(
            parse_github_version("v0.1.0-beta.1").unwrap(),
            Version::parse("0.1.0-beta.1").unwrap()
        );
        assert!(parse_github_version("not-a-version").is_none());
        assert!(parse_github_version("").is_none());
    }

    #[test]
    fn parses_latest_release_response() {
        let body = r#"{
            "tag_name": "v0.2.0",
            "html_url": "https://github.com/bindoffice/terry/releases/tag/v0.2.0",
            "name": "Terry 0.2.0",
            "published_at": "2026-08-30T10:00:00Z",
            "draft": false,
            "prerelease": false
        }"#;
        let info = parse_latest_release_response(body).unwrap();
        assert_eq!(info.version, Version::new(0, 2, 0));
        assert!(info.url.ends_with("/releases/tag/v0.2.0"));
        assert_eq!(info.title, "Terry 0.2.0");
        assert_eq!(info.published_at, "2026-08-30T10:00:00Z");
    }

    #[test]
    fn rejects_malformed_release_response() {
        assert!(parse_latest_release_response(r#"{"tag_name":"abc"}"#).is_err());
        assert!(parse_latest_release_response("not json").is_err());
    }

    #[test]
    fn transient_status_expires_after_deadline() {
        let mut state = UpdateCheckerState::default();
        assert!(matches!(state.status, UpdateStatus::Idle));

        state.set_status(UpdateStatus::UpToDate);
        assert!(state.transient_until.is_some());
        state.transient_until = Some(Instant::now() - Duration::from_secs(1));
        state.expire_transient();
        assert!(matches!(state.status, UpdateStatus::Idle));

        // Persistent statuses (available) never set a transient deadline, so
        // they survive `expire_transient` untouched.
        state.set_status(UpdateStatus::UpdateAvailable(UpdateInfo {
            version: Version::new(1, 0, 0),
            url: "https://example.com".into(),
            title: "release".into(),
            published_at: "now".into(),
        }));
        assert!(state.transient_until.is_none());
        state.expire_transient();
        assert!(matches!(state.status, UpdateStatus::UpdateAvailable(_)));
    }

    #[test]
    fn newest_release_wins_over_current_version() {
        let current = Version::new(0, 1, 0);
        let latest = parse_github_version("v0.2.0").unwrap();
        assert!(latest > current);

        let older = parse_github_version("v0.1.0").unwrap();
        assert!(!(older > current));

        let prerelease = parse_github_version("v0.2.0-beta.1").unwrap();
        assert!(!prerelease.pre.is_empty());
        assert!(prerelease > current);
        assert!(latest > prerelease);
    }
}
