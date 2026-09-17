use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::asset::{Candidate, HostArch, choose};
use crate::error::CoreError;
use crate::game::{GameId, RepositoryName};
use crate::release::{Channel, Download, Release};

const USER_AGENT: &str = "Turnstile";
const API_VERSION: &str = "2022-11-28";
const PER_PAGE: u32 = 100;

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// Parses a release list and resolves each one's download in the same pass.
/// A release with nothing this host can run is dropped: it is not an error,
/// it simply is not installable here.
pub fn parse_releases(
    json: &str,
    channel: Channel,
    host: HostArch,
) -> Result<Vec<Release>, CoreError> {
    let api: Vec<ApiRelease> =
        serde_json::from_str(json).map_err(|e| CoreError::Json(e.to_string()))?;

    let mut releases: Vec<Release> = api
        .into_iter()
        .filter_map(|r| {
            let candidates: Vec<Candidate> = r
                .assets
                .into_iter()
                .map(|a| Candidate {
                    name: a.name,
                    url: a.browser_download_url,
                    bytes: a.size,
                })
                .collect();
            let picked = choose(&candidates, host)?;
            Some(Release {
                tag: r.tag_name,
                published: r
                    .published_at
                    .as_deref()
                    .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok()),
                channel,
                download: Download {
                    url: picked.url.clone(),
                    bytes: picked.bytes,
                },
            })
        })
        .collect();

    Release::sort_newest_first(&mut releases);
    Ok(releases)
}

/// True when a page contains no published (non-prerelease) entry, which is
/// the only case where the `/releases/latest` fallback is worth a request.
fn page_has_no_stable_release(json: &str) -> bool {
    serde_json::from_str::<Vec<ApiRelease>>(json)
        .map(|v| !v.is_empty() && v.iter().all(|r| r.prerelease))
        .unwrap_or(false)
}

#[derive(Default)]
pub struct GitHub;

impl GitHub {
    pub fn new() -> GitHub {
        GitHub
    }

    /// The only place `ureq` is touched. If the 3.x surface differs from
    /// what is written here, this function is the only thing that changes.
    /// A GET against the public API, shared with `selfupdate`, which asks a
    /// different question of the same host and wants the same rate-limit and
    /// error handling.
    pub(crate) fn get_public(&self, url: &str) -> Result<String, CoreError> {
        self.get(url)
    }

    fn get(&self, url: &str) -> Result<String, CoreError> {
        let mut response = ureq::get(url)
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .call()
            .map_err(|e| match &e {
                ureq::Error::StatusCode(403) => CoreError::RateLimited,
                other => CoreError::Http(other.to_string()),
            })?;

        if response
            .headers()
            .get("x-ratelimit-remaining")
            .map(|v| v.as_bytes())
            == Some(b"0")
        {
            return Err(CoreError::RateLimited);
        }

        response
            .body_mut()
            .read_to_string()
            .map_err(|e| CoreError::Http(e.to_string()))
    }

    pub fn releases(
        &self,
        repo: &RepositoryName,
        channel: Channel,
        host: HostArch,
    ) -> Result<Vec<Release>, CoreError> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/releases?per_page={PER_PAGE}&page=1",
            repo.owner, repo.name
        );
        let body = self.get(&url)?;
        let mut releases = parse_releases(&body, channel, host)?;

        // Upstream fetches /releases/latest unconditionally because at its
        // page size of 75 a newer release could fall off page 1. At 100 that
        // should never happen, so the request is only made when the page
        // genuinely contains no stable release.
        if page_has_no_stable_release(&body) {
            let latest_url = format!(
                "https://api.github.com/repos/{}/{}/releases/latest",
                repo.owner, repo.name
            );
            if let Ok(latest_body) = self.get(&latest_url) {
                let wrapped = format!("[{latest_body}]");
                if let Ok(mut extra) = parse_releases(&wrapped, channel, host) {
                    extra.retain(|e| !releases.iter().any(|r| r.tag == e.tag));
                    releases.append(&mut extra);
                    Release::sort_newest_first(&mut releases);
                }
            }
        }

        Ok(releases)
    }

    pub fn releases_for(
        &self,
        game: GameId,
        include_develop: bool,
        host: HostArch,
    ) -> Result<Vec<Release>, CoreError> {
        let mut releases = self.releases(&game.release_repo(), Channel::Release, host)?;
        if include_develop && let Some(repo) = game.develop_repo() {
            releases.extend(self.releases(&repo, Channel::Develop, host)?);
        }
        Release::sort_newest_first(&mut releases);
        Ok(releases)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPENRCT2: &str = include_str!("../tests/fixtures/openrct2-releases.json");
    const DEVELOP: &str = include_str!("../tests/fixtures/openrct2-develop.json");
    const OPENLOCO: &str = include_str!("../tests/fixtures/openloco-releases.json");

    #[test]
    fn every_fixture_release_resolves_to_a_macos_download_on_apple_silicon() {
        for (name, json) in [
            ("openrct2", OPENRCT2),
            ("develop", DEVELOP),
            ("openloco", OPENLOCO),
        ] {
            let releases = parse_releases(json, Channel::Release, HostArch::Arm64).unwrap();
            assert!(!releases.is_empty(), "{name} produced no releases");
            for r in &releases {
                assert!(
                    r.download.url.starts_with("https://"),
                    "{name}: {}",
                    r.download.url
                );
                assert!(
                    r.download.url.to_lowercase().contains("macos"),
                    "{name} picked a non-macOS asset: {}",
                    r.download.url
                );
                assert!(r.download.bytes > 0, "{name}: zero-byte download");
            }
        }
    }

    #[test]
    fn openloco_yields_nothing_on_an_intel_mac() {
        // OpenLoco publishes an arm64-only macOS build.
        let releases = parse_releases(OPENLOCO, Channel::Release, HostArch::X86_64).unwrap();
        assert!(
            releases.is_empty(),
            "arm64-only builds must not be offered on Intel"
        );
    }

    #[test]
    fn openrct2_still_yields_releases_on_an_intel_mac() {
        // OpenRCT2 ships universal binaries.
        let releases = parse_releases(OPENRCT2, Channel::Release, HostArch::X86_64).unwrap();
        assert!(!releases.is_empty());
    }

    #[test]
    fn publication_timestamps_are_parsed() {
        let releases = parse_releases(OPENRCT2, Channel::Release, HostArch::Arm64).unwrap();
        assert!(releases.iter().any(|r| r.published.is_some()));
    }

    #[test]
    fn the_channel_is_applied_to_every_release() {
        let releases = parse_releases(DEVELOP, Channel::Develop, HostArch::Arm64).unwrap();
        assert!(releases.iter().all(|r| r.channel == Channel::Develop));
    }

    #[test]
    fn a_release_with_no_usable_asset_is_dropped_not_errored() {
        let json = r#"[
            {"tag_name":"v1","published_at":null,"prerelease":false,"assets":[
                {"name":"only-windows-x64.zip","browser_download_url":"https://x/w.zip","size":1}]},
            {"tag_name":"v2","published_at":null,"prerelease":false,"assets":[
                {"name":"game-macos-universal.zip","browser_download_url":"https://x/m.zip","size":2}]}
        ]"#;
        let releases = parse_releases(json, Channel::Release, HostArch::Arm64).unwrap();
        assert_eq!(
            releases.len(),
            1,
            "the unusable release should vanish, not error"
        );
        assert_eq!(releases[0].tag, "v2");
    }

    #[test]
    fn a_null_publication_date_parses() {
        let json = r#"[{"tag_name":"v1","published_at":null,"prerelease":false,"assets":[
            {"name":"g-macos.zip","browser_download_url":"https://x/m.zip","size":1}]}]"#;
        assert_eq!(
            parse_releases(json, Channel::Release, HostArch::Arm64).unwrap()[0].published,
            None
        );
    }

    #[test]
    fn malformed_json_is_an_error_not_a_panic() {
        assert!(parse_releases("{oh no", Channel::Release, HostArch::Arm64).is_err());
        assert!(
            parse_releases("[]", Channel::Release, HostArch::Arm64)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn releases_come_back_newest_first() {
        let releases = parse_releases(OPENRCT2, Channel::Release, HostArch::Arm64).unwrap();
        let dated: Vec<_> = releases.iter().filter_map(|r| r.published).collect();
        let mut sorted = dated.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(dated, sorted);
    }
}

#[cfg(test)]
mod network_tests {
    use super::*;

    #[test]
    #[ignore = "hits the live GitHub API; run with --ignored"]
    fn both_games_resolve_a_macos_download_from_the_live_api() {
        let gh = GitHub::new();
        for game in GameId::ALL {
            let releases = gh.releases_for(game, false, HostArch::Arm64).unwrap();
            assert!(
                !releases.is_empty(),
                "{} returned nothing",
                game.display_name()
            );
            assert!(releases[0].download.url.to_lowercase().contains("macos"));
        }
    }
}
