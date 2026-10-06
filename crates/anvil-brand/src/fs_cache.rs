//! Filesystem-backed `BrandCache` (`docs/BRANDING.md` §6): one JSON file
//! per tenant, and an `assets/` directory keyed by SHA-256 so tenants share
//! identical files. The asset directory is kept under a size budget, the
//! least recently used files going first, never one a cached profile names.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use anvil_core::{AnvilError, BrandAsset, BrandCache, BrandProfile};

/// The asset directory's budget (`docs/BRANDING.md` §6).
const DEFAULT_BUDGET: u64 = 50 * 1024 * 1024;

/// Cache rooted under the platform's cache dir (`~/.cache/anvil` on Linux,
/// `~/Library/Caches/anvil` on macOS, `%LOCALAPPDATA%\anvil\cache` on Windows).
pub struct FsBrandCache {
    root: PathBuf,
    budget: u64,
}

fn io(e: std::io::Error) -> AnvilError {
    AnvilError::Internal(format!("brand cache: {e}"))
}

/// A name safe for a file from any string: its hash.
fn file_key(s: &str) -> String {
    hex::encode(&Sha256::digest(s.as_bytes())[..16])
}

fn valid_hash(h: &str) -> bool {
    h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())
}

impl FsBrandCache {
    /// Cache in the default per-user location.
    pub fn new() -> Result<Self, AnvilError> {
        let base =
            dirs::cache_dir().ok_or_else(|| AnvilError::Config("no platform cache dir".into()))?;
        Ok(Self::at(base.join("anvil").join("brand")))
    }

    /// Cache in an explicit location. Useful for tests or portable installs.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            budget: DEFAULT_BUDGET,
        }
    }

    /// Keep the asset directory under `bytes`.
    pub fn with_budget(mut self, bytes: u64) -> Self {
        self.budget = bytes;
        self
    }

    /// Where the cache lives.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn profile_path(&self, tenant_id: &str) -> PathBuf {
        self.root
            .join("profiles")
            .join(format!("{}.json", file_key(tenant_id)))
    }

    fn account_path(&self, aor: &str) -> PathBuf {
        self.root.join("accounts").join(file_key(aor))
    }

    fn asset_path(&self, sha256: &str) -> PathBuf {
        self.root.join("assets").join(sha256.to_ascii_lowercase())
    }

    async fn write(path: &Path, bytes: &[u8]) -> Result<(), AnvilError> {
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await.map_err(io)?;
        }
        let partial = path.with_extension("partial");
        tokio::fs::write(&partial, bytes).await.map_err(io)?;
        tokio::fs::rename(&partial, path).await.map_err(io)
    }

    /// The asset's bytes into the store; its bytes out of the profile.
    async fn keep(&self, asset: &mut BrandAsset) -> Result<(), AnvilError> {
        if !valid_hash(&asset.sha256) {
            return Ok(());
        }
        let path = self.asset_path(&asset.sha256);
        if !asset.bytes.is_empty() && tokio::fs::metadata(&path).await.is_err() {
            Self::write(&path, &asset.bytes).await?;
        }
        asset.bytes.clear();
        asset.local_path = None;
        Ok(())
    }

    /// The asset's bytes back from the store, touched as used; `false` when
    /// the store lacks them.
    async fn restore(&self, asset: &mut BrandAsset) -> bool {
        if !valid_hash(&asset.sha256) {
            return false;
        }
        let path = self.asset_path(&asset.sha256);
        match tokio::fs::read(&path).await {
            Ok(bytes)
                if hex::encode(Sha256::digest(&bytes)).eq_ignore_ascii_case(&asset.sha256) =>
            {
                asset.bytes = bytes;
                asset.local_path = Some(path.to_string_lossy().into_owned());
                if let Ok(file) = std::fs::File::options().append(true).open(&path) {
                    let _ = file.set_modified(SystemTime::now());
                }
                true
            }
            _ => false,
        }
    }

    /// Every asset hash a cached profile names.
    async fn named(&self) -> std::collections::HashSet<String> {
        let mut named = std::collections::HashSet::new();
        let Ok(mut dir) = tokio::fs::read_dir(self.root.join("profiles")).await else {
            return named;
        };
        while let Ok(Some(entry)) = dir.next_entry().await {
            let Ok(text) = tokio::fs::read_to_string(entry.path()).await else {
                continue;
            };
            let Ok(p) = serde_json::from_str::<BrandProfile>(&text) else {
                continue;
            };
            for a in [&p.logo, &p.logo_dark, &p.icon].into_iter().flatten() {
                named.insert(a.sha256.to_ascii_lowercase());
            }
            for r in &p.ringtones {
                named.insert(r.asset.sha256.to_ascii_lowercase());
            }
        }
        named
    }

    /// Least recently used assets out until the directory fits its budget.
    async fn evict(&self) {
        let named = self.named().await;
        let Ok(mut dir) = tokio::fs::read_dir(self.root.join("assets")).await else {
            return;
        };
        let mut files = Vec::new();
        let mut total = 0u64;
        while let Ok(Some(entry)) = dir.next_entry().await {
            let Ok(meta) = entry.metadata().await else {
                continue;
            };
            total += meta.len();
            let name = entry.file_name().to_string_lossy().into_owned();
            if !named.contains(&name) {
                let used = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                files.push((used, meta.len(), entry.path()));
            }
        }
        files.sort();
        for (_, len, path) in files {
            if total <= self.budget {
                break;
            }
            if tokio::fs::remove_file(&path).await.is_ok() {
                total = total.saturating_sub(len);
            }
        }
    }
}

#[async_trait]
impl BrandCache for FsBrandCache {
    async fn load(&self, tenant_id: &str) -> Result<Option<BrandProfile>, AnvilError> {
        let text = match tokio::fs::read_to_string(self.profile_path(tenant_id)).await {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io(e)),
        };
        let mut p: BrandProfile = serde_json::from_str(&text)
            .map_err(|e| AnvilError::Internal(format!("cached brand: {e}")))?;
        // An asset the store lost is dropped, as a failed download is.
        for slot in [&mut p.logo, &mut p.logo_dark, &mut p.icon] {
            if let Some(a) = slot {
                if !self.restore(a).await {
                    *slot = None;
                }
            }
        }
        let mut ringtones = Vec::new();
        for mut r in std::mem::take(&mut p.ringtones) {
            if self.restore(&mut r.asset).await {
                ringtones.push(r);
            }
        }
        p.ringtones = ringtones;
        Ok(Some(p))
    }

    async fn store(&self, profile: &BrandProfile) -> Result<(), AnvilError> {
        let mut p = profile.clone();
        for a in [&mut p.logo, &mut p.logo_dark, &mut p.icon]
            .into_iter()
            .flatten()
        {
            self.keep(a).await?;
        }
        for r in &mut p.ringtones {
            self.keep(&mut r.asset).await?;
        }
        let json = serde_json::to_vec_pretty(&p)
            .map_err(|e| AnvilError::Internal(format!("brand cache: {e}")))?;
        Self::write(&self.profile_path(&p.tenant_id), &json).await?;
        self.evict().await;
        Ok(())
    }

    async fn clear(&self, tenant_id: &str) -> Result<(), AnvilError> {
        match tokio::fs::remove_file(self.profile_path(tenant_id)).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io(e)),
            _ => Ok(()),
        }
    }

    async fn last_tenant(&self, aor: &str) -> Result<Option<String>, AnvilError> {
        match tokio::fs::read_to_string(self.account_path(aor)).await {
            Ok(t) => Ok(Some(t.trim().to_string()).filter(|t| !t.is_empty())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(e)),
        }
    }

    async fn remember_tenant(&self, aor: &str, tenant_id: &str) -> Result<(), AnvilError> {
        Self::write(&self.account_path(aor), tenant_id.as_bytes()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(bytes: &[u8], mime: &str) -> BrandAsset {
        BrandAsset {
            url: "https://pbx.test/x".into(),
            mime: mime.into(),
            sha256: hex::encode(Sha256::digest(bytes)),
            bytes: bytes.to_vec(),
            width: None,
            height: None,
            local_path: None,
        }
    }

    fn profile(tenant: &str, logo: &[u8]) -> BrandProfile {
        let mut p: BrandProfile = serde_json::from_value(serde_json::json!({
            "schema_version": 1, "tenant_id": tenant, "app_name": "Acme", "etag": "e1",
        }))
        .unwrap();
        p.logo = Some(asset(logo, "image/png"));
        p
    }

    fn tempdir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("anvil-brand-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn a_profile_comes_back_with_its_files() {
        let dir = tempdir();
        let cache = FsBrandCache::at(&dir);
        assert!(cache.load("t1").await.unwrap().is_none());
        cache.store(&profile("t1", b"logo-bytes")).await.unwrap();
        cache.remember_tenant("sip:a@b", "t1").await.unwrap();
        assert_eq!(
            cache.last_tenant("sip:a@b").await.unwrap().as_deref(),
            Some("t1")
        );
        let back = cache.load("t1").await.unwrap().unwrap();
        let logo = back.logo.unwrap();
        assert_eq!(logo.bytes, b"logo-bytes");
        assert!(Path::new(logo.local_path.as_deref().unwrap()).exists());
        // The profile on disk holds no bytes.
        let raw = std::fs::read_to_string(cache.profile_path("t1")).unwrap();
        assert!(!raw.contains("\"bytes\""), "{raw}");
        cache.clear("t1").await.unwrap();
        assert!(cache.load("t1").await.unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn the_oldest_unnamed_files_go_first_and_named_ones_stay() {
        let dir = tempdir();
        let cache = FsBrandCache::at(&dir).with_budget(15);
        cache.store(&profile("t1", b"0123456789")).await.unwrap();
        // A second version of t1 names a new logo; the old one is unnamed.
        cache.store(&profile("t1", b"abcdefghij")).await.unwrap();
        let old = cache.asset_path(&hex::encode(Sha256::digest(b"0123456789")));
        let new = cache.asset_path(&hex::encode(Sha256::digest(b"abcdefghij")));
        assert!(!old.exists(), "over budget: the unnamed file went");
        assert!(new.exists(), "the named file stays");
        let _ = std::fs::remove_dir_all(dir);
    }
}
