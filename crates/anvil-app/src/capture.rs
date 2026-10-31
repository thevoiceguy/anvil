//! Capturing calls (FCP's `docs/CALL_DIAGNOSTICS.md` §3): only while the
//! user's organisation allows this app, and only when the user turns it on —
//! for the next call or every call. A capture is kept in the app's folder
//! (at most 200 MB in all, 7 days each) and uploaded to FCP when its call
//! ends, retried while the app runs.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

/// The most the folder keeps, unless one capture may be larger.
const KEEP_BYTES: u64 = 200 * 1024 * 1024;
/// How long a capture is kept.
const KEEP_FOR: Duration = Duration::from_secs(7 * 86_400);
/// The waits between upload attempts; then it is given up.
const RETRIES: [Duration; 5] = [
    Duration::from_secs(30),
    Duration::from_secs(120),
    Duration::from_secs(600),
    Duration::from_secs(1800),
    Duration::from_secs(3600),
];

/// Which calls the user captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    #[default]
    Off,
    /// The next call, then off again.
    NextCall,
    /// Every call until turned off.
    EveryCall,
}

/// What the app shows of captures.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CaptureState {
    /// FCP allows this app to capture (the organisation, then an
    /// administrator); the setting shows only then.
    pub allowed: bool,
    /// Until when, RFC 3339.
    pub until: Option<String>,
    /// The largest capture, in MB.
    pub max_mb: u32,
    /// What the user chose; it takes effect only while allowed.
    pub mode: CaptureMode,
    /// The captures kept on this device, and their size.
    pub saved: u32,
    pub saved_bytes: u64,
    /// Where they are kept.
    pub folder: Option<String>,
}

/// The captures' folder: beside the settings.
pub(crate) fn folder(settings_path: Option<&Path>) -> Option<PathBuf> {
    Some(settings_path?.parent()?.join("captures"))
}

/// The captures in `dir`, oldest first: path, size, modified.
fn list(dir: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "pcapng"))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some((e.path(), meta.len(), meta.modified().ok()?))
        })
        .collect();
    out.sort_by_key(|(_, _, at)| *at);
    out
}

/// How many captures `dir` keeps, and their size.
pub(crate) fn summary(dir: &Path) -> (u32, u64) {
    let all = list(dir);
    (all.len() as u32, all.iter().map(|(_, len, _)| len).sum())
}

/// Remove captures older than a week, then the oldest until the rest fit
/// in `keep_bytes`; `spare` is never removed (the one just taken).
pub(crate) fn prune(dir: &Path, keep_bytes: u64, spare: Option<&Path>, now: SystemTime) {
    let mut all = list(dir);
    all.retain(|(path, _, at)| {
        let old = now.duration_since(*at).unwrap_or_default() > KEEP_FOR;
        if old && Some(path.as_path()) != spare {
            let _ = std::fs::remove_file(path);
            return false;
        }
        true
    });
    let mut total: u64 = all.iter().map(|(_, len, _)| len).sum();
    for (path, len, _) in &all {
        if total <= keep_bytes {
            break;
        }
        if Some(path.as_path()) != spare && std::fs::remove_file(path).is_ok() {
            total -= len;
        }
    }
}

/// The folder's limit for captures of at most `max_mb`.
pub(crate) fn keep_bytes(max_mb: u32) -> u64 {
    KEEP_BYTES.max(u64::from(max_mb) * 1024 * 1024)
}

/// Remove every capture in `dir`.
pub(crate) fn delete_all(dir: &Path) {
    for (path, _, _) in list(dir) {
        let _ = std::fs::remove_file(path);
    }
}

/// Whether FCP's refusal is final: not allowed, too large, not the user's
/// call, not a capture.
fn is_final(e: &anvil_fcp::FcpError) -> bool {
    matches!(
        e,
        anvil_fcp::FcpError::Refused {
            status: 400 | 403 | 404 | 413,
            ..
        } | anvil_fcp::FcpError::SignedOut
    )
}

/// Upload a finished capture, retried a few times while the app runs; the
/// local copy is kept either way.
pub(crate) async fn upload(
    fcp: std::sync::Arc<anvil_fcp::FcpClient>,
    device_id: String,
    server_call: Option<String>,
    path: PathBuf,
) {
    let mut waits = RETRIES.iter();
    loop {
        let body = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(e) => {
                tracing::debug!(%e, path = %path.display(), "a capture to upload is gone");
                return;
            }
        };
        match fcp
            .upload_capture(&device_id, server_call.as_deref(), body)
            .await
        {
            Ok(()) => {
                tracing::info!(path = %path.display(), "capture uploaded");
                return;
            }
            Err(e) if is_final(&e) => {
                tracing::info!(%e, path = %path.display(), "capture not uploaded");
                return;
            }
            Err(e) => match waits.next() {
                Some(wait) => {
                    tracing::debug!(%e, ?wait, "capture upload failed; trying again");
                    tokio::time::sleep(*wait).await;
                }
                None => {
                    tracing::info!(%e, path = %path.display(), "capture upload given up");
                    return;
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_folder_keeps_a_week_and_its_limit_never_the_newest() {
        let dir = std::env::temp_dir().join(format!("anvil-app-captures-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = |name: &str, len: usize, age_days: u64| {
            let path = dir.join(name);
            std::fs::write(&path, vec![0u8; len]).unwrap();
            let at = SystemTime::now() - Duration::from_secs(age_days * 86_400 + 60);
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(at)
                .unwrap();
            path
        };
        file("a-old.pcapng", 10, 8);
        file("b.pcapng", 400, 3);
        file("c.pcapng", 400, 2);
        let newest = file("d.pcapng", 400, 0);
        std::fs::write(dir.join("notes.txt"), b"not a capture").unwrap();
        assert_eq!(summary(&dir), (4, 1210));
        prune(&dir, 900, Some(&newest), SystemTime::now());
        let left: Vec<String> = list(&dir)
            .into_iter()
            .map(|(p, _, _)| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, vec!["c.pcapng", "d.pcapng"]);
        delete_all(&dir);
        assert_eq!(summary(&dir), (0, 0));
        assert!(dir.join("notes.txt").exists(), "only captures");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(keep_bytes(50), KEEP_BYTES);
        assert_eq!(keep_bytes(500), 500 * 1024 * 1024);
    }
}
