//! Filesystem-backed `BrandCache`. Stores one JSON file per tenant plus an
//! `assets/` directory keyed by sha256 so multiple tenants can share common
//! assets (e.g. the FCP default ringtone). Phase 2 target — stub today.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use anvil_core::{AnvilError, BrandCache, BrandProfile};

/// Cache rooted under the platform's cache dir (`~/.cache/anvil` on Linux,
/// `~/Library/Caches/anvil` on macOS, `%LOCALAPPDATA%\anvil\cache` on Windows).
pub struct FsBrandCache {
    root: PathBuf,
}

impl FsBrandCache {
    /// Cache in the default per-user location.
    pub fn new() -> Result<Self, AnvilError> {
        let base =
            dirs::cache_dir().ok_or_else(|| AnvilError::Config("no platform cache dir".into()))?;
        Ok(Self {
            root: base.join("anvil").join("brand"),
        })
    }

    /// Cache in an explicit location. Useful for tests or portable installs.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Where the cache lives.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[async_trait]
impl BrandCache for FsBrandCache {
    async fn load(&self, _tenant_id: &str) -> Result<Option<BrandProfile>, AnvilError> {
        Ok(None) // Phase 2
    }

    async fn store(&self, _profile: &BrandProfile) -> Result<(), AnvilError> {
        Ok(()) // Phase 2
    }

    async fn clear(&self, _tenant_id: &str) -> Result<(), AnvilError> {
        Ok(()) // Phase 2
    }
}
