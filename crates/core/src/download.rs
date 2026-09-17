use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::CoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Downloading,
    Extracting,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    pub status: Status,
    pub value: Option<f64>,
}

/// Streams `url` into `dest`. `progress` is called with a fractional value
/// when the server reports a length and with `None` when it does not, which
/// the UI renders as an indeterminate bar. A partial file is never left
/// behind on failure or cancellation.
pub fn download_to_temp(
    url: &str,
    dest: &Path,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<(), CoreError> {
    match stream(url, dest, progress, cancel) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(dest);
            Err(e)
        }
    }
}

fn stream(
    url: &str,
    dest: &Path,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<(), CoreError> {
    progress(Progress {
        status: Status::Downloading,
        value: Some(0.0),
    });

    let mut response = ureq::get(url)
        .header("User-Agent", "Turnstile")
        .call()
        .map_err(|e| CoreError::Http(e.to_string()))?;

    let total: Option<u64> = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());

    if total.is_none() {
        progress(Progress {
            status: Status::Downloading,
            value: None,
        });
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::Io(e.to_string()))?;
    }

    let mut reader = response.body_mut().as_reader();
    let mut file = File::create(dest).map_err(|e| CoreError::Io(e.to_string()))?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut downloaded: u64 = 0;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
        let read = reader
            .read(&mut buf)
            .map_err(|e| CoreError::Http(e.to_string()))?;
        if read == 0 {
            break;
        }
        file.write_all(&buf[..read])
            .map_err(|e| CoreError::Io(e.to_string()))?;
        downloaded += read as u64;
        if let Some(total) = total.filter(|t| *t > 0) {
            let fraction = (downloaded as f64 / total as f64).clamp(0.0, 1.0);
            progress(Progress {
                status: Status::Downloading,
                value: Some(fraction),
            });
        }
    }

    file.sync_all().map_err(|e| CoreError::Io(e.to_string()))?;
    progress(Progress {
        status: Status::Downloading,
        value: Some(1.0),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::AtomicBool;

    /// Serves one response, then exits. Returns the bound URL.
    fn serve(body: Vec<u8>, send_content_length: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file.zip", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut discard = [0u8; 1024];
            let _ = stream.read(&mut discard);
            let mut head = String::from("HTTP/1.1 200 OK\r\n");
            if send_content_length {
                head.push_str(&format!("Content-Length: {}\r\n", body.len()));
            } else {
                head.push_str("Connection: close\r\n");
            }
            head.push_str("\r\n");
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        });
        url
    }

    fn tempdir() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("turnstile-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn a_file_is_downloaded_byte_for_byte() {
        let body = vec![7u8; 5000];
        let url = serve(body.clone(), true);
        let dest = tempdir().join("out.bin");
        let cancel = AtomicBool::new(false);
        download_to_temp(&url, &dest, &mut |_| {}, &cancel).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
    }

    #[test]
    fn progress_reaches_one_when_content_length_is_known() {
        let url = serve(vec![0u8; 5000], true);
        let dest = tempdir().join("out2.bin");
        let cancel = AtomicBool::new(false);
        let mut seen: Vec<Option<f64>> = Vec::new();
        download_to_temp(&url, &dest, &mut |p| seen.push(p.value), &cancel).unwrap();
        assert_eq!(seen.first(), Some(&Some(0.0)));
        assert_eq!(seen.last(), Some(&Some(1.0)));
        assert!(
            seen.iter()
                .all(|v| v.map(|x| (0.0..=1.0).contains(&x)).unwrap_or(true))
        );
    }

    #[test]
    fn progress_is_indeterminate_when_content_length_is_absent() {
        let url = serve(vec![0u8; 2000], false);
        let dest = tempdir().join("out3.bin");
        let cancel = AtomicBool::new(false);
        let mut seen: Vec<Option<f64>> = Vec::new();
        download_to_temp(&url, &dest, &mut |p| seen.push(p.value), &cancel).unwrap();
        assert!(
            seen.iter().any(|v| v.is_none()),
            "expected an indeterminate report"
        );
    }

    #[test]
    fn cancellation_stops_the_download_and_removes_the_partial_file() {
        let url = serve(vec![0u8; 5_000_000], true);
        let dest = tempdir().join("out4.bin");
        let cancel = AtomicBool::new(true); // cancelled before the first chunk
        let err = download_to_temp(&url, &dest, &mut |_| {}, &cancel).unwrap_err();
        assert!(matches!(err, CoreError::Cancelled));
        assert!(
            !dest.exists(),
            "a cancelled download must not leave a partial file"
        );
    }

    #[test]
    fn the_status_is_always_downloading_from_this_function() {
        let url = serve(vec![0u8; 100], true);
        let dest = tempdir().join("out5.bin");
        let cancel = AtomicBool::new(false);
        let mut statuses = Vec::new();
        download_to_temp(&url, &dest, &mut |p| statuses.push(p.status), &cancel).unwrap();
        assert!(statuses.iter().all(|s| *s == Status::Downloading));
    }
}
