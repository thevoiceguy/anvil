//! The app updating itself (`docs/APP.md` §7, U3b).
//!
//! Each release carries `latest.json`, a [`Manifest`] naming the release's
//! files with their sizes and SHA-256, and `latest.json.sig`, an Ed25519
//! signature over it made in the release workflow with the key held as the
//! repository's `ANVIL_UPDATE_KEY` secret. [`Updater::check`] reads both
//! from the latest release and accepts the manifest only under
//! [`UPDATE_KEY`], the public half built in; [`Updater::download`] fetches
//! this platform's file and checks it against the manifest;
//! [`Updater::install`] installs it the way this copy was installed
//! ([`Install`]) once the app has quit, and starts it again.

#![forbid(unsafe_code)]

mod install;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

pub use install::Install;

/// The public key releases are signed with (Ed25519, raw). Its private half
/// is the repository's `ANVIL_UPDATE_KEY` secret; `app/packaging/update-key.pub.pem`
/// is the same key, which the release workflow checks each signature against.
pub const UPDATE_KEY: [u8; 32] = [
    0x56, 0xfb, 0x48, 0x61, 0x18, 0x44, 0xf4, 0x8d, 0xc5, 0xe5, 0x13, 0xee, 0x68, 0xe1, 0x73, 0x16,
    0xd9, 0xdb, 0x74, 0xc9, 0xb9, 0x06, 0xa0, 0x5e, 0xfd, 0x1a, 0x53, 0xb0, 0x9e, 0xbb, 0xca, 0x4c,
];

/// Where the latest release's manifest is.
pub const FEED: &str = "https://github.com/thevoiceguy/anvil/releases/latest/download/";

/// What can go wrong updating.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("the update server: {0}")]
    Http(#[from] reqwest::Error),
    #[error("the manifest's signature is not Anvil's")]
    BadSignature,
    #[error("the manifest cannot be read: {0}")]
    BadManifest(String),
    #[error("the download is not the release's file: {0}")]
    BadFile(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    CannotInstall(String),
}

/// A release, as its `latest.json` describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// `X.Y.Z`.
    pub version: String,
    /// The release's page, for its notes.
    pub notes: String,
    /// Its files by target: `windows-x86_64-setup`, `macos-dmg`,
    /// `linux-x86_64-appimage`, `linux-amd64-deb`.
    pub files: BTreeMap<String, File>,
}

/// One of a release's files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    pub name: String,
    pub url: String,
    /// Lowercase hex.
    pub sha256: String,
    pub size: u64,
}

/// The manifest, when `signature` is `key`'s over exactly these bytes.
pub fn verify_manifest(
    bytes: &[u8],
    signature: &[u8],
    key: &[u8],
) -> Result<Manifest, UpdateError> {
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(bytes, signature)
        .map_err(|_| UpdateError::BadSignature)?;
    let manifest: Manifest =
        serde_json::from_slice(bytes).map_err(|e| UpdateError::BadManifest(e.to_string()))?;
    semver::Version::parse(&manifest.version)
        .map_err(|e| UpdateError::BadManifest(format!("version {:?}: {e}", manifest.version)))?;
    Ok(manifest)
}

/// A newer release than this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    pub manifest: Manifest,
    /// This copy's file in it, when this copy can install one.
    pub file: Option<File>,
}

impl Available {
    pub fn version(&self) -> &str {
        &self.manifest.version
    }
}

/// Checks for, fetches and installs updates for this copy of the app.
pub struct Updater {
    http: reqwest::Client,
    feed: String,
    key: Vec<u8>,
    current: semver::Version,
    install: Install,
}

impl Updater {
    /// For this copy (`current` is its version), against the releases.
    pub fn new(current: &str) -> Result<Self, UpdateError> {
        Self::with(current, FEED, &UPDATE_KEY, Install::this())
    }

    /// Against another feed and key: for tests.
    pub fn with(
        current: &str,
        feed: &str,
        key: &[u8],
        install: Install,
    ) -> Result<Self, UpdateError> {
        let current = semver::Version::parse(current)
            .map_err(|e| UpdateError::BadManifest(format!("this version {current:?}: {e}")))?;
        let http = reqwest::Client::builder()
            .user_agent(concat!("anvil-update/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(600))
            .build()?;
        Ok(Self {
            http,
            feed: if feed.ends_with('/') {
                feed.to_string()
            } else {
                format!("{feed}/")
            },
            key: key.to_vec(),
            current,
            install,
        })
    }

    /// How this copy was installed, and so how it updates.
    pub fn install_kind(&self) -> &Install {
        &self.install
    }

    /// The latest release, when it is newer than this copy.
    pub async fn check(&self) -> Result<Option<Available>, UpdateError> {
        let bytes = self.fetch("latest.json").await?;
        let signature = self.fetch("latest.json.sig").await?;
        let manifest = verify_manifest(&bytes, &signature, &self.key)?;
        let latest = semver::Version::parse(&manifest.version).expect("verified");
        if latest <= self.current {
            return Ok(None);
        }
        let file = self
            .install
            .target()
            .and_then(|t| manifest.files.get(t).cloned());
        Ok(Some(Available { manifest, file }))
    }

    async fn fetch(&self, name: &str) -> Result<Vec<u8>, UpdateError> {
        let url = format!("{}{name}", self.feed);
        let response = self.http.get(&url).send().await?.error_for_status()?;
        Ok(response.bytes().await?.to_vec())
    }

    /// This copy's file of `available` into `dir`, checked against the
    /// manifest's size and SHA-256 (a file that is not is deleted).
    pub async fn download(
        &self,
        available: &Available,
        dir: &Path,
    ) -> Result<PathBuf, UpdateError> {
        let file = available
            .file
            .as_ref()
            .ok_or_else(|| UpdateError::CannotInstall("this copy cannot update itself".into()))?;
        if file.name.contains(['/', '\\']) || file.name.starts_with('.') {
            return Err(UpdateError::BadFile(format!("name {:?}", file.name)));
        }
        tokio::fs::create_dir_all(dir).await?;
        let path = dir.join(&file.name);
        let partial = dir.join(format!("{}.part", file.name));
        let response = self.http.get(&file.url).send().await?.error_for_status()?;
        let mut out = tokio::fs::File::create(&partial).await?;
        let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
        let mut size = 0u64;
        let mut body = response.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk = chunk?;
            size += chunk.len() as u64;
            if size > file.size {
                break;
            }
            digest.update(&chunk);
            out.write_all(&chunk).await?;
        }
        out.flush().await?;
        drop(out);
        let sha256 = hex(digest.finish().as_ref());
        if size != file.size || sha256 != file.sha256 {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(UpdateError::BadFile(if size != file.size {
                format!("{size} bytes, not {}", file.size)
            } else {
                format!("SHA-256 {sha256}, not {}", file.sha256)
            }));
        }
        tokio::fs::rename(&partial, &path).await?;
        Ok(path)
    }

    /// Install `file` (from [`download`](Self::download)) once this process
    /// has exited, then start the app again. The caller quits right after.
    pub fn install(&self, file: &Path) -> Result<(), UpdateError> {
        self.install.start(file, std::process::id())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_by_openssl_is_accepted_and_a_changed_byte_is_not() {
        // Made with `openssl pkeyutl -sign -rawin` and a throwaway key, as
        // the release workflow signs.
        let manifest = include_bytes!("../tests/fixtures/latest.json");
        let signature = include_bytes!("../tests/fixtures/latest.json.sig");
        let key = include_bytes!("../tests/fixtures/key.raw");
        let m = verify_manifest(manifest, signature, key).unwrap();
        assert_eq!(m.version, "9.8.7");
        assert_eq!(m.files["linux-x86_64-appimage"].size, 3);

        let mut changed = manifest.to_vec();
        let i = changed.iter().position(|&b| b == b'9').unwrap();
        changed[i] = b'1';
        assert!(matches!(
            verify_manifest(&changed, signature, key),
            Err(UpdateError::BadSignature)
        ));
        assert!(matches!(
            verify_manifest(manifest, signature, &UPDATE_KEY),
            Err(UpdateError::BadSignature)
        ));
    }
}

#[cfg(test)]
mod key_tests {
    /// The key built in is the one the release workflow checks signatures
    /// against (`app/packaging/update-key.pub.pem`).
    #[test]
    fn the_key_built_in_is_the_release_workflows() {
        let pem = include_str!("../../../app/packaging/update-key.pub.pem");
        let b64: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
        let der = base64(&b64);
        assert_eq!(der.len(), 44, "an Ed25519 SubjectPublicKeyInfo");
        assert_eq!(der[der.len() - 32..], super::UPDATE_KEY);
    }

    fn base64(text: &str) -> Vec<u8> {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::new();
        let (mut acc, mut bits) = (0u32, 0);
        for c in text.bytes().filter(|&c| c != b'=') {
            let v = ALPHABET.iter().position(|&a| a == c).expect("base64") as u32;
            acc = (acc << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
            }
        }
        out
    }
}
