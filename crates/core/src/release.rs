use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Release,
    Develop,
}

/// The file we would fetch for this release on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    pub url: String,
    pub bytes: u64,
}

/// A release this host can actually install. One carrying no macOS build it
/// can run is filtered out while parsing, so there is no asset-selection step
/// at install time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub published: Option<OffsetDateTime>,
    pub channel: Channel,
    pub download: Download,
}

impl Release {
    /// Newest first, undated last. Publication time is the only ordering:
    /// develop tags such as `v0.5.5-9-g394e588fc7` are not semantic versions
    /// and nothing in the application compares versions numerically.
    pub fn sort_newest_first(releases: &mut [Release]) {
        releases.sort_by(|a, b| match (a.published, b.published) {
            (Some(x), Some(y)) => y.cmp(&x),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn rel(tag: &str, at: Option<time::OffsetDateTime>) -> Release {
        Release {
            tag: tag.to_string(),
            published: at,
            channel: Channel::Release,
            download: Download {
                url: "https://example.invalid/a.zip".into(),
                bytes: 1,
            },
        }
    }

    #[test]
    fn releases_sort_newest_first() {
        let mut v = vec![
            rel("old", Some(datetime!(2024-01-01 00:00 UTC))),
            rel("new", Some(datetime!(2026-01-01 00:00 UTC))),
            rel("mid", Some(datetime!(2025-01-01 00:00 UTC))),
        ];
        Release::sort_newest_first(&mut v);
        assert_eq!(
            v.iter().map(|r| r.tag.as_str()).collect::<Vec<_>>(),
            ["new", "mid", "old"]
        );
    }

    #[test]
    fn releases_without_a_publication_date_sort_last() {
        let mut v = vec![
            rel("undated", None),
            rel("dated", Some(datetime!(2020-01-01 00:00 UTC))),
        ];
        Release::sort_newest_first(&mut v);
        assert_eq!(v[0].tag, "dated");
        assert_eq!(v[1].tag, "undated");
    }

    #[test]
    fn sorting_is_stable_for_equal_timestamps() {
        let at = Some(datetime!(2025-01-01 00:00 UTC));
        let mut v = vec![rel("first", at), rel("second", at)];
        Release::sort_newest_first(&mut v);
        assert_eq!(
            v.iter().map(|r| r.tag.as_str()).collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    #[test]
    fn a_release_always_carries_a_download() {
        let r = rel("v1", None);
        assert!(r.download.url.ends_with(".zip"));
    }

    #[test]
    fn channels_are_distinguishable() {
        let mut r = rel("v1", None);
        assert_eq!(r.channel, Channel::Release);
        r.channel = Channel::Develop;
        assert_ne!(r.channel, Channel::Release);
    }
}
