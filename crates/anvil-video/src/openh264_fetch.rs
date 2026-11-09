//! Cisco's OpenH264 binary, fetched at first use (FCP's
//! `docs/SOFTPHONE_VIDEO.md` §7 Q4).
//!
//! H.264 is patented, and Cisco pays the royalties for its own binary of
//! OpenH264 only when each user downloads it from Cisco, as Firefox does.
//! So Anvil never ships it or builds it: on Linux the binary is fetched
//! from Cisco into the user's data directory, checked against the SHA-256
//! pinned here (and again by the `openh264` crate when it is loaded), and
//! used from there. Until it arrives, calls offer VP8.
//!
//! The platforms with hardware H.264 (macOS, Windows, the phones) do not
//! use it.

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// What an app using it must show (Cisco's terms).
pub const NOTICE: &str = "OpenH264 Video Codec provided by Cisco Systems, Inc.";

/// One build of Cisco's binary.
#[derive(Debug, Clone, Copy)]
pub struct Binary {
    /// Where Cisco publishes it, compressed with bzip2.
    pub url: &'static str,
    /// Its name on disk.
    pub file: &'static str,
    /// The SHA-256 of the library itself (uncompressed).
    pub sha256: &'static str,
}

/// This platform's build, where Anvil uses one.
pub fn binary() -> Option<Binary> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(Binary {
            url: "https://ciscobinary.openh264.org/libopenh264-2.6.0-linux64.8.so.bz2",
            file: "libopenh264-2.6.0-linux64.8.so",
            sha256: "2f0cde7c6a6abcf5cae76942894ea42897fa677bce4ed6c91a24dd1b041d5f04",
        })
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some(Binary {
            url: "https://ciscobinary.openh264.org/libopenh264-2.6.0-linux-arm64.8.so.bz2",
            file: "libopenh264-2.6.0-linux-arm64.8.so",
            sha256: "12e7b33623667cdab0e575170c147b1b36eadb77d0d2aa7ceb5afd3e58902140",
        })
    } else {
        None
    }
}

/// The largest download accepted (the library is ~0.6 MB compressed).
const MAX_DOWNLOAD: u64 = 8 << 20;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The library in `dir`, if it is there and is Cisco's.
pub fn installed(dir: &Path) -> Option<PathBuf> {
    let bin = binary()?;
    let path = dir.join(bin.file);
    let bytes = std::fs::read(&path).ok()?;
    (hex(&Sha256::digest(&bytes)) == bin.sha256).then_some(path)
}

/// The library in `dir`, fetched from Cisco if it is not there yet.
pub async fn fetch(dir: &Path) -> Result<PathBuf, String> {
    if let Some(path) = installed(dir) {
        return Ok(path);
    }
    let bin = binary().ok_or("no OpenH264 build for this platform")?;
    tracing::info!(url = bin.url, "fetching Cisco's OpenH264");
    let response = reqwest::get(bin.url)
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("OpenH264 download: {e}"))?;
    if response.content_length().unwrap_or(0) > MAX_DOWNLOAD {
        return Err("OpenH264 download: too large".into());
    }
    let compressed = response
        .bytes()
        .await
        .map_err(|e| format!("OpenH264 download: {e}"))?;
    if compressed.len() as u64 > MAX_DOWNLOAD {
        return Err("OpenH264 download: too large".into());
    }
    let mut library = Vec::new();
    bzip2::read::BzDecoder::new(&compressed[..])
        .take(4 * MAX_DOWNLOAD)
        .read_to_end(&mut library)
        .map_err(|e| format!("OpenH264 download: {e}"))?;
    let sha = hex(&Sha256::digest(&library));
    if sha != bin.sha256 {
        return Err(format!(
            "OpenH264 download: SHA-256 {sha}, not Cisco's {}",
            bin.sha256
        ));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(bin.file);
    let partial = dir.join(format!("{}.partial", bin.file));
    std::fs::write(&partial, &library)
        .and_then(|()| std::fs::rename(&partial, &path))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    tracing::info!(path = %path.display(), "OpenH264 installed");
    Ok(path)
}
