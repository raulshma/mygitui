//! Release update check (`app.check_updates` palette command).
//!
//! Queries the GitHub releases API for the latest published tag and
//! compares it against the running binary's version. Purely informational:
//! the result surfaces as a notice, and the release URL is copyable — no
//! auto-download, no auto-restart.

use serde::Deserialize;

const RELEASES_API: &str = "https://api.github.com/repos/raulshma/mygitui/releases/latest";

#[derive(Debug, Deserialize)]
struct LatestRelease {
    tag_name: String,
    html_url: String,
}

/// Outcome of a check, ready for a notice.
#[derive(Debug, Clone)]
pub enum Update {
    /// Already on the newest published release.
    UpToDate(String),
    /// A newer tag exists: (latest tag, release URL).
    Available(String, String),
}

/// Queries the latest release; network errors become the `Err` side.
pub async fn check() -> Result<Update, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .user_agent("mygitui")
        .build()
        .map_err(|e| format!("client build failed: {e}"))?;
    let release: LatestRelease = client
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("release lookup failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("release lookup failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("release parse failed: {e}"))?;

    let current = env!("CARGO_PKG_VERSION");
    if is_newer(&release.tag_name, current) {
        Ok(Update::Available(release.tag_name, release.html_url))
    } else {
        Ok(Update::UpToDate(current.to_string()))
    }
}

/// Tag/semver comparison for `vMAJOR.MINOR.PATCH` tags: parse the numeric
/// triple after an optional `v` prefix and compare component-wise. Tags
/// that do not fit the shape never count as newer (pre-release tags like
/// `v0.2.0-rc1` compare by their numeric prefix only).
fn is_newer(tag: &str, current: &str) -> bool {
    let (Some(tag), Some(cur)) = (parse_version(tag), parse_version(current)) else {
        return false;
    };
    tag > cur
}

/// `(major, minor, patch)` of a `v?MAJOR.MINOR.PATCH…` string, ignoring
/// anything after the third numeric component.
fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let digits: String = version.trim().trim_start_matches(['v', 'V']).to_string();
    let mut parts = digits.split('.');
    let major: u64 = parts.next()?.parse().ok()?;
    let minor: u64 = parts.next().unwrap_or("0").parse().ok()?;
    let patch: String = parts
        .next()
        .unwrap_or("0")
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let patch: u64 = if patch.is_empty() {
        0
    } else {
        patch.parse().ok()?
    };
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing_covers_tags_and_suffixes() {
        assert_eq!(parse_version("0.1.12"), Some((0, 1, 12)));
        assert_eq!(parse_version("v0.1.12"), Some((0, 1, 12)));
        assert_eq!(parse_version("V0.2.0-rc1"), Some((0, 2, 0)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("oops"), None);
    }

    #[test]
    fn newer_detects_growing_versions() {
        assert!(is_newer("v0.1.13", "0.1.12"));
        assert!(is_newer("0.2.0", "0.1.99"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("v0.1.12", "0.1.12"));
        assert!(!is_newer("0.1.11", "0.1.12"));
        assert!(!is_newer("weird-tag", "0.1.12"));
    }
}
