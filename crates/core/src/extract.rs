use std::path::Path;
use std::process::Command;

use crate::error::CoreError;

/// Extracts `archive` into `into`. Dispatch is on `url_path`, not on the
/// archive's own filename, because the downloaded file is a temporary name
/// with no extension.
///
/// Zips go through `/usr/bin/ditto` rather than a Rust zip crate: ditto
/// preserves the extended attributes and code signatures inside a `.app`,
/// and a bundle extracted without them is one Gatekeeper refuses to launch.
pub fn extract(archive: &Path, url_path: &str, into: &Path) -> Result<(), CoreError> {
    let lower = url_path.to_ascii_lowercase();
    std::fs::create_dir_all(into).map_err(|e| CoreError::Io(e.to_string()))?;

    if lower.ends_with(".zip") {
        run(Command::new("/usr/bin/ditto")
            .arg("-x")
            .arg("-k")
            .arg(archive)
            .arg(into))
    } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        run(Command::new("tar")
            .arg("-C")
            .arg(into)
            .arg("-xf")
            .arg(archive))?;
        flatten_single_directory(into)
    } else {
        Err(CoreError::UnknownArchiveFormat)
    }
}

fn run(cmd: &mut Command) -> Result<(), CoreError> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let output = cmd
        .output()
        .map_err(|e| CoreError::ExtractionFailed(format!("could not run {program}: {e}")))?;
    if !output.status.success() {
        return Err(CoreError::ExtractionFailed(format!(
            "{program} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// A tarball that contains exactly one top-level directory is flattened so
/// the game sits directly in the destination, matching what the zips do.
fn flatten_single_directory(into: &Path) -> Result<(), CoreError> {
    let entries: Vec<_> = std::fs::read_dir(into)
        .map_err(|e| CoreError::Io(e.to_string()))?
        .flatten()
        .collect();
    if entries.len() != 1 || !entries[0].path().is_dir() {
        return Ok(());
    }
    let wrapper = entries[0].path();
    for entry in std::fs::read_dir(&wrapper)
        .map_err(|e| CoreError::Io(e.to_string()))?
        .flatten()
    {
        let dest = into.join(entry.file_name());
        std::fs::rename(entry.path(), dest).map_err(|e| CoreError::Io(e.to_string()))?;
    }
    std::fs::remove_dir(&wrapper).map_err(|e| CoreError::Io(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn scratch(name: &str) -> std::path::PathBuf {
        let p =
            std::env::temp_dir().join(format!("turnstile-extract-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// Builds a real zip with ditto, which is what the app extracts with.
    fn make_zip(dir: &std::path::Path, payload_name: &str) -> std::path::PathBuf {
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join(payload_name), b"hello").unwrap();
        let zip = dir.join("archive.zip");
        let status = Command::new("/usr/bin/ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(&src)
            .arg(&zip)
            .status()
            .unwrap();
        assert!(status.success());
        zip
    }

    #[test]
    fn a_zip_is_extracted_with_ditto() {
        let d = scratch("zip");
        let zip = make_zip(&d, "payload.txt");
        let out = d.join("out");
        extract(&zip, "/downloads/OpenRCT2-macos.zip", &out).unwrap();
        let found = walk(&out);
        assert!(
            found.iter().any(|p| p.ends_with("payload.txt")),
            "found {found:?}"
        );
    }

    #[test]
    fn a_tar_gz_with_a_single_top_level_directory_is_flattened() {
        let d = scratch("tar");
        let inner = d.join("build/OpenRCT2-1.0");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join("openrct2"), b"bin").unwrap();
        let tgz = d.join("archive.tar.gz");
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&tgz)
            .arg("-C")
            .arg(d.join("build"))
            .arg("OpenRCT2-1.0")
            .status()
            .unwrap();
        assert!(status.success());

        let out = d.join("out");
        extract(&tgz, "/downloads/OpenRCT2-linux.tar.gz", &out).unwrap();
        assert!(out.join("openrct2").exists(), "found {:?}", walk(&out));
        assert!(!out.join("OpenRCT2-1.0").exists());
    }

    #[test]
    fn an_unknown_extension_is_rejected() {
        let d = scratch("unknown");
        let f = d.join("archive.rar");
        std::fs::write(&f, b"x").unwrap();
        let err = extract(&f, "/downloads/thing.rar", &d.join("out")).unwrap_err();
        assert!(matches!(err, CoreError::UnknownArchiveFormat));
    }

    #[test]
    fn a_corrupt_zip_reports_extraction_failure_rather_than_succeeding() {
        let d = scratch("corrupt");
        let f = d.join("archive.zip");
        std::fs::write(&f, b"this is not a zip file at all").unwrap();
        let err = extract(&f, "/downloads/thing.zip", &d.join("out")).unwrap_err();
        assert!(matches!(err, CoreError::ExtractionFailed(_)));
    }

    #[test]
    fn the_extension_comes_from_the_url_not_the_temp_filename() {
        let d = scratch("urlext");
        let zip = make_zip(&d, "payload.txt");
        let renamed = d.join("tmp-no-extension");
        std::fs::rename(&zip, &renamed).unwrap();
        let out = d.join("out");
        extract(&renamed, "/downloads/OpenRCT2-macos.zip", &out).unwrap();
        assert!(walk(&out).iter().any(|p| p.ends_with("payload.txt")));
    }

    fn walk(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(p) = stack.pop() {
            if let Ok(rd) = std::fs::read_dir(&p) {
                for e in rd.flatten() {
                    let path = e.path();
                    if path.is_dir() {
                        stack.push(path.clone());
                    }
                    out.push(path);
                }
            }
        }
        out
    }
}
