//! Update checker that polls the GitHub Releases API. A newer release is only
//! downloaded after the user clicks the update button.

mod download;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use futures::AsyncReadExt;
use gpui::{actions, App, Context, EventEmitter, Global, Subscription, Window};
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

/// A file attached to a GitHub release.
#[derive(Clone, Debug)]
pub struct ReleaseAsset {
    /// File name, for example `Terry-0.2.0-macos-aarch64.zip`.
    pub name: String,
    /// Direct download URL.
    pub download_url: String,
    /// Size in bytes, when GitHub reported one.
    pub size: u64,
}

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
    /// Files published with the release.
    pub assets: Vec<ReleaseAsset>,
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
    /// A newer version is available. Nothing is downloaded until the user clicks.
    UpdateAvailable(UpdateInfo),
    /// The user clicked update and the package is downloading.
    Downloading {
        /// Release being downloaded.
        info: UpdateInfo,
        /// Download progress in `0.0..=1.0`, when the server sent a content length.
        progress: Option<f32>,
    },
    /// The new build replaced the running app. Restart to finish.
    RestartRequired {
        /// Release that was installed.
        info: UpdateInfo,
    },
    /// The package is on disk. This process was not a packaged install, or
    /// replacing it failed.
    PackageReady {
        /// Release that was downloaded.
        info: UpdateInfo,
        /// Path of the downloaded archive.
        path: std::path::PathBuf,
    },
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
/// Date tags such as `v20261001` become `20261001.0.0` so they compare in
/// calendar order.
pub fn parse_github_version(tag: &str) -> Option<Version> {
    let tag = tag.strip_prefix('v').unwrap_or(tag);
    if let Some(date) = date_version(tag) {
        return Some(Version::new(date, 0, 0));
    }
    Version::parse(tag).ok()
}

/// `YYYYMMDD` when `version` was produced from a date tag, otherwise the
/// semver text.
pub fn display_version(version: &Version) -> String {
    if version.minor == 0
        && version.patch == 0
        && version.pre.is_empty()
        && (10_000_000..100_000_000).contains(&version.major)
    {
        version.major.to_string()
    } else {
        version.to_string()
    }
}

fn date_version(tag: &str) -> Option<u64> {
    let bytes = tag.as_bytes();
    if bytes.len() != 8 || !bytes.iter().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let month: u8 = tag[4..6].parse().ok()?;
    let day: u8 = tag[6..8].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    tag.parse().ok()
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
        assets: release
            .assets
            .into_iter()
            .map(|asset| ReleaseAsset {
                name: asset.name,
                download_url: asset.browser_download_url,
                size: asset.size,
            })
            .collect(),
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
        matches!(
            state.status,
            UpdateStatus::Checking | UpdateStatus::Downloading { .. }
        )
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

/// Downloads and installs the release the user clicked. Checks never call this.
pub fn download_update(cx: &mut App) {
    let info = match &cx.global::<UpdateCheckerState>().status {
        UpdateStatus::UpdateAvailable(info) => info.clone(),
        UpdateStatus::PackageReady { path, .. } => {
            cx.open_with_system(path);
            return;
        }
        UpdateStatus::RestartRequired { .. } => {
            restart_to_update(cx);
            return;
        }
        _ => return,
    };

    let Some(asset) = download::select_release_asset(
        &info.assets,
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
    .cloned() else {
        if !info.url.is_empty() {
            cx.open_url(&info.url);
        }
        return;
    };

    cx.update_global::<UpdateCheckerState, _>(|state, _| {
        state.set_status(UpdateStatus::Downloading {
            info: info.clone(),
            progress: None,
        });
    });

    let progress = Arc::new(AtomicU32::new(0));
    let progress_reader = progress.clone();
    let (tx, rx) = mpsc::channel::<Result<download::InstallOutcome>>();
    let http_client = cx.http_client();
    let background = cx.background_executor().clone();
    let version = info.version.clone();

    background
        .spawn(async move {
            let result = download::fetch_and_install(&asset, http_client.as_ref(), |fraction| {
                let encoded = fraction
                    .map(|value| (value * 1000.0) as u32)
                    .unwrap_or(u32::MAX);
                progress.store(encoded, Ordering::Relaxed);
            })
            .await;
            let _ = tx.send(result);
        })
        .detach();

    let background = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        loop {
            background.timer(Duration::from_millis(200)).await;
            let encoded = progress_reader.load(Ordering::Relaxed);
            let finished = rx.try_recv();
            let keep_going = cx.update(|cx| {
                    let still_downloading = matches!(
                        &cx.global::<UpdateCheckerState>().status,
                        UpdateStatus::Downloading { info, .. } if info.version == version
                    );
                    if !still_downloading {
                        return false;
                    }
                    if encoded != 0 && encoded != u32::MAX {
                        let fraction = encoded as f32 / 1000.0;
                        cx.update_global::<UpdateCheckerState, _>(|state, _| {
                            if let UpdateStatus::Downloading { progress, .. } = &mut state.status {
                                *progress = Some(fraction);
                            }
                        });
                    }
                    match finished {
                        Ok(Ok(download::InstallOutcome::RestartRequired)) => {
                            let info = info.clone();
                            cx.update_global::<UpdateCheckerState, _>(|state, _| {
                                state.set_status(UpdateStatus::RestartRequired { info });
                            });
                            false
                        }
                        Ok(Ok(download::InstallOutcome::PackageReady(path))) => {
                            cx.open_with_system(&path);
                            let info = info.clone();
                            cx.update_global::<UpdateCheckerState, _>(|state, _| {
                                state.set_status(UpdateStatus::PackageReady { info, path });
                            });
                            false
                        }
                        Ok(Err(error)) => {
                            cx.update_global::<UpdateCheckerState, _>(|state, _| {
                                state.set_status(UpdateStatus::Failed(format!("{error:#}")));
                            });
                            false
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            cx.update_global::<UpdateCheckerState, _>(|state, _| {
                                state.set_status(UpdateStatus::Failed(
                                    "update download stopped".into(),
                                ));
                            });
                            false
                        }
                        Err(mpsc::TryRecvError::Empty) => true,
                    }
                });
            if !keep_going {
                break;
            }
        }
    })
    .detach();
}

fn restart_to_update(cx: &mut App) {
    if let Err(error) = download::schedule_relaunch() {
        cx.update_global::<UpdateCheckerState, _>(|state, _| {
            state.set_status(UpdateStatus::Failed(format!("{error:#}")));
        });
        return;
    }
    cx.quit();
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
            UpdateStatus::Idle => {
                let version = display_version(&AppVersion::global(cx));
                Button::new("app-version", version)
                    .label_size(LabelSize::Small)
                    .tooltip(Tooltip::text(i18n::t("check_for_updates")))
                    .on_click(|_, _window, cx| check_for_updates(cx))
                    .into_any_element()
            }
            UpdateStatus::Checking => Button::new(
                "update-checking",
                i18n::t("checking_for_updates"),
            )
            .label_size(LabelSize::Small)
            .loading(true)
            .into_any_element(),
            UpdateStatus::UpdateAvailable(release) => Button::new(
                "update-available",
                display_version(&release.version),
            )
            .style(ButtonStyle::Filled)
            .label_size(LabelSize::Small)
            .start_icon(Icon::new(IconName::CloudDownload))
            .tooltip(Tooltip::text(format!(
                "{} {}",
                i18n::t("update_available"),
                release.title
            )))
            .on_click(|_, _window, cx| download_update(cx))
            .into_any_element(),
            UpdateStatus::Downloading { progress, .. } => {
                let label = match progress {
                    Some(fraction) => format!(
                        "{} {}%",
                        i18n::t("downloading_update"),
                        (fraction * 100.0) as u8
                    ),
                    None => i18n::t("downloading_update"),
                };
                Button::new("update-downloading", label)
                    .label_size(LabelSize::Small)
                    .loading(true)
                    .into_any_element()
            }
            UpdateStatus::RestartRequired { info } => Button::new(
                "update-restart",
                i18n::t("restart_to_update"),
            )
            .style(ButtonStyle::Filled)
            .label_size(LabelSize::Small)
            .tooltip(Tooltip::text(display_version(&info.version)))
            .on_click(|_, _window, cx| restart_to_update(cx))
            .into_any_element(),
            UpdateStatus::PackageReady { info, path } => {
                let path = path.clone();
                Button::new("update-package-ready", i18n::t("open_update_package"))
                    .style(ButtonStyle::Filled)
                    .label_size(LabelSize::Small)
                    .tooltip(Tooltip::text(display_version(&info.version)))
                    .on_click(move |_, _window, cx| {
                        cx.open_with_system(&path);
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

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
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
    #[serde(default)]
    assets: Vec<GitHubAsset>,
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
        assert_eq!(
            parse_github_version("v20261001").unwrap(),
            Version::new(20261001, 0, 0)
        );
        assert_eq!(
            parse_github_version("20261002").unwrap(),
            Version::new(20261002, 0, 0)
        );
        assert!(parse_github_version("v20261301").is_none());
        assert!(parse_github_version("v20261001").unwrap() < parse_github_version("20261002").unwrap());
        assert_eq!(
            display_version(&Version::new(20261001, 0, 0)),
            "20261001"
        );
        assert_eq!(display_version(&Version::new(1, 2, 3)), "1.2.3");
    }

    #[test]
    fn parses_latest_release_response() {
        let body = r#"{
            "tag_name": "v0.2.0",
            "html_url": "https://github.com/bindoffice/terry/releases/tag/v0.2.0",
            "name": "Terry 0.2.0",
            "published_at": "2026-08-30T10:00:00Z",
            "draft": false,
            "prerelease": false,
            "assets": [{
                "name": "Terry-0.2.0-macos-aarch64.zip",
                "browser_download_url": "https://example.com/Terry-0.2.0-macos-aarch64.zip",
                "size": 42
            }]
        }"#;
        let info = parse_latest_release_response(body).unwrap();
        assert_eq!(info.version, Version::new(0, 2, 0));
        assert!(info.url.ends_with("/releases/tag/v0.2.0"));
        assert_eq!(info.title, "Terry 0.2.0");
        assert_eq!(info.published_at, "2026-08-30T10:00:00Z");
        assert_eq!(info.assets.len(), 1);
        assert_eq!(info.assets[0].name, "Terry-0.2.0-macos-aarch64.zip");
        assert_eq!(info.assets[0].size, 42);
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
            assets: Vec::new(),
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
