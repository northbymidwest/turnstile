//! Whether a newer Turnstile has been published.
//!
//! Separate from `github::releases`, which exists to find a game build this
//! host can run: that path drops any release with nothing installable, and
//! Turnstile ships a `.dmg`, which it does not recognise. Reusing it would
//! have reported "up to date" forever.
//!
//! This reads two fields and downloads nothing. The banner it feeds opens the
//! releases page; replacing a running, notarized bundle in place is a
//! different and much riskier feature.

use serde::Deserialize;

use crate::error::CoreError;
use crate::github::GitHub;

/// A published release newer than the version asking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub version: String,
    pub url: String,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
}

/// Parses `/releases/latest` and reports an update only if the published
/// version is strictly greater than `current`, so a local build ahead of the
/// feed does not nag and a moved or recut tag cannot announce itself as an
/// upgrade. A tag this cannot parse is ignored for the same reason.
pub fn parse_latest(json: &str, current: &str) -> Result<Option<Update>, CoreError> {
    let api: ApiRelease = serde_json::from_str(json).map_err(|e| CoreError::Json(e.to_string()))?;
    let Some(published) = semver(api.tag_name.trim_start_matches('v')) else {
        return Ok(None);
    };
    // Stripped on both sides: `current` carries no prefix today, but accepting
    // it on one side and not the other is a trap for a later caller.
    let Some(running) = semver(current.trim_start_matches('v')) else {
        return Ok(None);
    };
    Ok((published > running).then(|| Update {
        version: api.tag_name.trim_start_matches('v').to_string(),
        url: api.html_url,
    }))
}

/// `major.minor.patch` and nothing else. A pre-release or build suffix makes
/// this `None`, which is how pre-releases are ignored without a second rule.
fn semver(s: &str) -> Option<(u64, u64, u64)> {
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

impl GitHub {
    /// Asks whether a release newer than `current` exists. `/releases/latest`
    /// rather than the list: GitHub excludes drafts and pre-releases from it.
    pub fn latest_turnstile(
        &self,
        owner: &str,
        repo: &str,
        current: &str,
    ) -> Result<Option<Update>, CoreError> {
        let url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
        parse_latest(&self.get_public(&url)?, current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(tag: &str) -> String {
        format!(r#"{{"tag_name":"{tag}","html_url":"https://example.invalid/releases/{tag}"}}"#)
    }

    #[test]
    fn a_newer_release_is_an_update() {
        let u = parse_latest(&body("v0.2.0"), "0.1.0").unwrap().unwrap();
        assert_eq!(u.version, "0.2.0");
        assert_eq!(u.url, "https://example.invalid/releases/v0.2.0");
    }

    #[test]
    fn the_same_version_is_not_an_update() {
        assert_eq!(parse_latest(&body("v0.1.0"), "0.1.0").unwrap(), None);
    }

    #[test]
    fn an_older_release_is_not_an_update() {
        assert_eq!(parse_latest(&body("v0.1.0"), "0.2.0").unwrap(), None);
    }

    #[test]
    fn the_v_prefix_is_optional_on_either_side() {
        assert!(parse_latest(&body("0.2.0"), "0.1.0").unwrap().is_some());
        assert!(parse_latest(&body("v0.2.0"), "v0.1.0").unwrap().is_some());
    }

    #[test]
    fn each_component_is_compared_as_a_number_not_a_string() {
        assert!(parse_latest(&body("v0.10.0"), "0.9.0").unwrap().is_some());
        assert_eq!(parse_latest(&body("v0.9.0"), "0.10.0").unwrap(), None);
    }

    #[test]
    fn a_tag_that_cannot_be_compared_is_ignored() {
        for tag in ["nightly", "v1.2", "v1.2.3.4", "v1.2.3-rc1", ""] {
            assert_eq!(
                parse_latest(&body(tag), "0.1.0").unwrap(),
                None,
                "{tag} should not announce an update"
            );
        }
    }

    #[test]
    fn a_running_version_that_cannot_be_parsed_announces_nothing() {
        assert_eq!(
            parse_latest(&body("v9.9.9"), "not-a-version").unwrap(),
            None
        );
    }

    #[test]
    fn a_malformed_body_is_an_error_rather_than_a_silent_no() {
        assert!(parse_latest("{", "0.1.0").is_err());
    }
}
