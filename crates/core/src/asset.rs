#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetPlatform {
    MacOS,
    Windows,
    Linux,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetArch {
    Arm64,
    X86_64,
    Universal,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostArch {
    Arm64,
    X86_64,
}

impl HostArch {
    pub fn current() -> HostArch {
        if cfg!(target_arch = "aarch64") {
            HostArch::Arm64
        } else {
            HostArch::X86_64
        }
    }
}

/// One release asset, before we decide whether we want it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    pub url: String,
    pub bytes: u64,
}

impl Candidate {
    fn has(&self, needle: &str) -> bool {
        self.name.to_ascii_lowercase().contains(needle)
    }

    pub fn platform(&self) -> AssetPlatform {
        if self.has("macos") || self.has("osx") || self.has("darwin") {
            AssetPlatform::MacOS
        } else if self.has("windows") || self.has("win32") || self.has("winnt") || self.has(".exe")
        {
            AssetPlatform::Windows
        } else if self.has("linux") || self.has(".appimage") || self.has(".deb") || self.has(".rpm")
        {
            AssetPlatform::Linux
        } else {
            AssetPlatform::Unknown
        }
    }

    /// Longer names first: `x86_64` must never be read as `x86`, and
    /// `aarch64` must not fall through to the generic branch.
    pub fn arch(&self) -> AssetArch {
        if self.has("arm64") || self.has("aarch64") {
            AssetArch::Arm64
        } else if self.has("x86_64") || self.has("x64") || self.has("amd64") || self.has("intel") {
            AssetArch::X86_64
        } else if self.has("universal") {
            AssetArch::Universal
        } else {
            AssetArch::Unknown
        }
    }

    /// An archive that is a game build, for a platform that might be ours.
    ///
    /// An `Unknown` platform is accepted rather than discarded: a future
    /// release might name a macOS build in a way we do not recognise, and
    /// rejecting it outright would make the game silently uninstallable.
    pub fn is_usable(&self) -> bool {
        let is_archive = self.has(".zip") || self.has(".tar.gz") || self.has(".tgz");
        let is_build = !(self.has("symbols")
            || self.has("installer")
            || self.has("debug")
            || self.has("source"));
        let platform_ok = matches!(
            self.platform(),
            AssetPlatform::MacOS | AssetPlatform::Unknown
        );
        is_archive && is_build && platform_ok
    }

    pub fn runs_on(&self, host: HostArch) -> bool {
        match (self.arch(), host) {
            (AssetArch::Arm64, HostArch::X86_64) => false,
            // Intel builds run on Apple Silicon under Rosetta; universal and
            // unrecognised builds are assumed to run anywhere.
            _ => true,
        }
    }

    /// Lower sorts first.
    pub fn rank(&self, host: HostArch) -> (u8, u8) {
        let platform = match self.platform() {
            AssetPlatform::MacOS => 0,
            _ => 1,
        };
        let arch = match (self.arch(), host) {
            (AssetArch::Arm64, HostArch::Arm64) => 0,
            (AssetArch::X86_64, HostArch::X86_64) => 0,
            (AssetArch::Universal, _) => 1,
            (AssetArch::Unknown, _) => 2,
            // Anything left runs only through translation.
            _ => 3,
        };
        (platform, arch)
    }
}

/// The best download for this host, or `None` when the release offers
/// nothing runnable. `None` is a normal outcome, not an error: OpenLoco
/// publishes no Intel macOS build.
pub fn choose(candidates: &[Candidate], host: HostArch) -> Option<&Candidate> {
    candidates
        .iter()
        .filter(|c| c.is_usable() && c.runs_on(host))
        .min_by_key(|c| c.rank(host))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str) -> Candidate {
        Candidate {
            name: name.to_string(),
            url: format!("https://example.invalid/{name}"),
            bytes: 1,
        }
    }

    #[test]
    fn macos_is_recognised_under_several_spellings() {
        assert_eq!(
            c("OpenRCT2-macos-universal.zip").platform(),
            AssetPlatform::MacOS
        );
        assert_eq!(c("OpenRCT2-osx.zip").platform(), AssetPlatform::MacOS);
        assert_eq!(
            c("OpenRCT2-darwin-arm64.zip").platform(),
            AssetPlatform::MacOS
        );
    }

    #[test]
    fn other_platforms_are_recognised_so_they_can_be_excluded() {
        assert_eq!(
            c("OpenLoco-windows-portable-x64.zip").platform(),
            AssetPlatform::Windows
        );
        assert_eq!(
            c("OpenRCT2-windows-installer-x64.exe").platform(),
            AssetPlatform::Windows
        );
        assert_eq!(
            c("OpenLoco-linux-x64-glibc2.43.tar.gz").platform(),
            AssetPlatform::Linux
        );
        assert_eq!(
            c("OpenRCT2-linux-x86_64.AppImage").platform(),
            AssetPlatform::Linux
        );
    }

    #[test]
    fn an_unrecognised_platform_is_unknown_not_guessed() {
        assert_eq!(c("OpenRCT2-v1.0.zip").platform(), AssetPlatform::Unknown);
    }

    #[test]
    fn architecture_sniffing_tests_longer_names_first() {
        assert_eq!(c("game-macos-arm64.zip").arch(), AssetArch::Arm64);
        assert_eq!(c("game-macos-aarch64.zip").arch(), AssetArch::Arm64);
        assert_eq!(c("game-macos-x86_64.zip").arch(), AssetArch::X86_64);
        assert_eq!(c("game-macos-x64.zip").arch(), AssetArch::X86_64);
        assert_eq!(c("game-macos-universal.zip").arch(), AssetArch::Universal);
        assert_eq!(c("game-macos.zip").arch(), AssetArch::Unknown);
    }

    #[test]
    fn symbols_installers_and_sources_are_not_usable() {
        assert!(!c("OpenRCT2-macos-symbols.zip").is_usable());
        assert!(!c("OpenRCT2-macos-installer.zip").is_usable());
        assert!(!c("OpenRCT2-macos-debug.zip").is_usable());
        assert!(
            !c("source-code.zip").is_usable(),
            "source archives are not builds"
        );
    }

    #[test]
    fn only_archive_formats_are_usable() {
        assert!(c("OpenRCT2-macos-universal.zip").is_usable());
        assert!(c("OpenRCT2-macos.tar.gz").is_usable());
        assert!(!c("OpenRCT2-sha256sums.txt").is_usable());
        assert!(!c("OpenRCT2-android.apk").is_usable());
    }

    #[test]
    fn other_platforms_are_not_usable_even_as_archives() {
        assert!(!c("OpenLoco-windows-portable-x64.zip").is_usable());
        assert!(!c("OpenLoco-linux-x64-glibc2.43.tar.gz").is_usable());
    }

    #[test]
    fn an_arm64_build_does_not_run_on_intel() {
        // Real case: OpenLoco publishes no Intel macOS build at all.
        let a = c("OpenLoco-v26.08-macos-arm64.zip");
        assert!(a.runs_on(HostArch::Arm64));
        assert!(!a.runs_on(HostArch::X86_64));
    }

    #[test]
    fn an_intel_build_runs_everywhere_under_rosetta() {
        let a = c("OpenRCT2-macos-x86_64.zip");
        assert!(a.runs_on(HostArch::X86_64));
        assert!(a.runs_on(HostArch::Arm64));
    }

    #[test]
    fn universal_and_unknown_builds_are_assumed_to_run_anywhere() {
        for name in ["OpenRCT2-macos-universal.zip", "OpenRCT2-macos.zip"] {
            assert!(c(name).runs_on(HostArch::Arm64), "{name}");
            assert!(c(name).runs_on(HostArch::X86_64), "{name}");
        }
    }

    #[test]
    fn the_real_release_asset_lists_resolve_to_the_macos_build() {
        let openrct2 = [
            c("OpenRCT2-v0.5.5-9-g394e588fc7-android.apk"),
            c("OpenRCT2-v0.5.5-9-g394e588fc7-Linux-bookworm-x86_64.tar.gz"),
            c("OpenRCT2-v0.5.5-9-g394e588fc7-linux-x86_64.AppImage"),
            c("OpenRCT2-v0.5.5-9-g394e588fc7-macos-universal.zip"),
            c("OpenRCT2-v0.5.5-9-g394e588fc7-sha256sums.txt"),
            c("OpenRCT2-v0.5.5-9-g394e588fc7-windows-installer-x64.exe"),
            c("OpenRCT2-v0.5.5-9-g394e588fc7-windows-portable-x64.zip"),
            c("OpenRCT2-v0.5.5-9-g394e588fc7-windows-symbols-x64.zip"),
        ];
        let picked = choose(&openrct2, HostArch::Arm64).unwrap();
        assert_eq!(
            picked.name,
            "OpenRCT2-v0.5.5-9-g394e588fc7-macos-universal.zip"
        );
    }

    #[test]
    fn openloco_has_nothing_installable_on_an_intel_mac() {
        let openloco = [
            c("OpenLoco-v26.08-linux-x64-glibc2.43.tar.gz"),
            c("OpenLoco-v26.08-macos-arm64.zip"),
            c("OpenLoco-v26.08-windows-portable-x64.zip"),
            c("OpenLoco-v26.08-windows-x64-symbols.zip"),
        ];
        assert_eq!(
            choose(&openloco, HostArch::Arm64).map(|c| c.name.as_str()),
            Some("OpenLoco-v26.08-macos-arm64.zip")
        );
        assert!(
            choose(&openloco, HostArch::X86_64).is_none(),
            "an arm64-only build must not be offered on Intel"
        );
    }

    #[test]
    fn a_future_split_build_picks_the_host_architecture() {
        // Neither project ships this today. The ranking exists so that if
        // either stops shipping universal binaries, nothing has to change.
        let split = [
            c("OpenRCT2-v9.9-macos-x86_64.zip"),
            c("OpenRCT2-v9.9-macos-arm64.zip"),
        ];
        assert_eq!(
            choose(&split, HostArch::Arm64).map(|c| c.name.as_str()),
            Some("OpenRCT2-v9.9-macos-arm64.zip")
        );
        assert_eq!(
            choose(&split, HostArch::X86_64).map(|c| c.name.as_str()),
            Some("OpenRCT2-v9.9-macos-x86_64.zip")
        );
    }

    #[test]
    fn an_exact_match_beats_universal_which_beats_translation() {
        let all = [
            c("OpenRCT2-macos-x86_64.zip"),
            c("OpenRCT2-macos-universal.zip"),
            c("OpenRCT2-macos-arm64.zip"),
        ];
        assert_eq!(
            choose(&all, HostArch::Arm64).map(|c| c.name.as_str()),
            Some("OpenRCT2-macos-arm64.zip")
        );
    }

    #[test]
    fn a_recognised_macos_asset_beats_an_unrecognised_one() {
        let all = [
            c("OpenRCT2-v1.0.zip"),
            c("OpenRCT2-v1.0-macos-universal.zip"),
        ];
        assert_eq!(
            choose(&all, HostArch::Arm64).map(|c| c.name.as_str()),
            Some("OpenRCT2-v1.0-macos-universal.zip")
        );
    }

    #[test]
    fn a_release_with_no_usable_asset_chooses_nothing() {
        let none = [c("OpenRCT2-sha256sums.txt"), c("OpenRCT2-windows-x64.zip")];
        assert!(choose(&none, HostArch::Arm64).is_none());
        assert!(choose(&[], HostArch::Arm64).is_none());
    }
}
