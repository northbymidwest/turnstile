//! Domain logic for Turnstile. No Objective-C and no `unsafe`. Environment
//! access is confined to `Dirs::system()`; every other filesystem root is
//! injected, so this crate tests headlessly.

pub mod age;
pub mod asset;
pub mod dirs;
pub mod download;
pub mod error;
pub mod extract;
pub mod game;
pub mod gamedata;
pub mod github;
pub mod install;
pub mod release;
pub mod selfupdate;
pub mod store;

pub use age::{Age, now};
pub use asset::{AssetArch, AssetPlatform, Candidate, HostArch, choose};
pub use dirs::Dirs;
pub use download::{Progress, Status, download_to_temp};
pub use error::{CoreError, StoreError, TagError};
pub use extract::extract;
pub use game::{GameId, RepositoryName, sanitize_tag, version_file_in};
pub use gamedata::{OriginalGame, identify, install_game_data, installed_at};
pub use github::{GitHub, parse_releases};
pub use install::{install, launch};
pub use release::{Channel, Download, Release};
pub use store::{DisableReport, Installed, Mode, UNKNOWN_TAG, VersionStore};
