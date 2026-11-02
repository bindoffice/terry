//! Where `tom` lives upstream.
//!
//! Every URL that points at the project's own repository is derived from the
//! constants here, so pointing a build at a different home (a fork, an internal
//! mirror, an air-gapped enterprise install) is a one-line change instead of a
//! grep across `updates.rs`, `prebuilt_release.rs` and `changelog.rs`.
//!
//! The two non-default entry points keep their environment overrides, because a
//! running binary can be redirected without being rebuilt:
//!
//! - `TOM_CHANGELOG_URL` overrides [`changelog_url`].
//! - `TOM_RELEASES_URL` overrides [`releases_api`].
//!
//! There is deliberately no override for [`REPOSITORY`]: it is the trust anchor
//! for prebuilt downloads. `prebuilt_release` verifies that a release manifest
//! and every asset URL it names belong to this exact repository, so letting the
//! environment move the anchor would let a hostile manifest point at itself.

/// The project's home on GitHub, without a trailing slash.
///
/// Used to build release download URLs and to validate that a manifest and its
/// assets come from this repository and no other.
pub const REPOSITORY: &str = "https://github.com/solos/tom";

/// The owner/name pair, for API paths that take them separately.
pub const REPOSITORY_SLUG: &str = "solos/tom";

/// The branch the documentation and changelog are read from.
pub const DEFAULT_BRANCH: &str = "main";

/// All releases, newest first, as GitHub returns them.
///
/// The caller filters out drafts and prereleases; the list endpoint is used
/// rather than the "latest" one so a prerelease channel can see its own tags.
pub fn releases_api() -> String {
    std::env::var("TOM_RELEASES_URL").unwrap_or_else(|_| {
        format!("https://api.github.com/repos/{REPOSITORY_SLUG}/releases?per_page=100")
    })
}

/// The single newest non-draft, non-prerelease release.
///
/// Authoritative version source for the stable channel.
pub fn releases_latest_api() -> String {
    match std::env::var("TOM_RELEASES_URL") {
        // An override naming a release list cannot also answer "which one is
        // latest", so it applies to this endpoint too: point both at the same
        // place rather than half-redirecting the updater.
        Ok(base) if !base.is_empty() => latest_from(&base),
        _ => format!("https://api.github.com/repos/{REPOSITORY_SLUG}/releases/latest"),
    }
}

/// Derives the "latest release" endpoint from a release-list override.
///
/// A list URL ends in `/releases` or `/releases?...`; the latest endpoint is
/// the same path with `/latest` appended and any query dropped.
fn latest_from(list_url: &str) -> String {
    let without_query = list_url.split('?').next().unwrap_or(list_url);
    let trimmed = without_query.trim_end_matches('/');
    if trimmed.ends_with("/releases") {
        format!("{trimmed}/latest")
    } else {
        // The caller pointed somewhere that is not a release list at all;
        // pass it through unchanged rather than inventing a path.
        trimmed.to_string()
    }
}

/// The changelog, read from the default branch.
pub fn changelog_url() -> String {
    std::env::var("TOM_CHANGELOG_URL").unwrap_or_else(|_| {
        format!("https://raw.githubusercontent.com/{REPOSITORY_SLUG}/{DEFAULT_BRANCH}/CHANGELOG.md")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_urls_all_point_at_one_repository() {
        // The point of this module: no URL may drift to another owner.
        for url in [releases_api(), releases_latest_api(), changelog_url()] {
            assert!(
                url.contains(REPOSITORY_SLUG),
                "{url} does not point at {REPOSITORY_SLUG}"
            );
        }
    }

    #[test]
    fn the_latest_endpoint_is_derived_from_the_list_endpoint() {
        assert_eq!(
            latest_from("https://api.github.com/repos/solos/tom/releases?per_page=100"),
            "https://api.github.com/repos/solos/tom/releases/latest"
        );
        // A trailing slash must not produce `//latest`.
        assert_eq!(
            latest_from("https://example.test/releases/"),
            "https://example.test/releases/latest"
        );
        // Something that is not a release list is passed through, not mangled.
        assert_eq!(
            latest_from("https://mirror.test/tom"),
            "https://mirror.test/tom"
        );
    }

    #[test]
    fn the_repository_has_no_trailing_slash() {
        // `REPOSITORY` is concatenated with `/releases/...` in several places;
        // a trailing slash would produce `//releases`.
        assert!(!REPOSITORY.ends_with('/'));
        assert!(REPOSITORY.ends_with(REPOSITORY_SLUG));
    }
}
