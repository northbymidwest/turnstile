use std::path::PathBuf;

/// Filesystem root for game installations. Always injected so tests can
/// redirect the whole crate at a temporary directory.
#[derive(Debug, Clone)]
pub struct Dirs {
    pub app_support: PathBuf,
}

impl Dirs {
    /// `~/Library/Application Support`. The only environment access in this
    /// crate.
    pub fn system() -> Option<Dirs> {
        let home = std::env::home_dir()?;
        Some(Dirs {
            app_support: home.join("Library/Application Support"),
        })
    }

    pub fn at(root: impl Into<PathBuf>) -> Dirs {
        Dirs {
            app_support: root.into(),
        }
    }
}
