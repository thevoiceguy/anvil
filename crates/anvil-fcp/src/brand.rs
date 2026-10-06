//! The tenant's brand from FCP (`docs/BRANDING.md`): `anvil-brand`'s
//! provider, given the app session's access token for each fetch, fresh
//! (an access token lasts minutes; a brand is fetched again for hours).

use std::sync::Arc;

use anvil_brand::{FsBrandCache, HttpBrandProvider};
use anvil_core::{
    AnvilError, BrandCache, BrandConfig, BrandCredential, BrandFetchOutcome, BrandProvider,
    BrandRequest,
};
use async_trait::async_trait;

use crate::FcpClient;

/// A [`BrandProvider`] that signs each fetch with the session's token.
pub struct FcpBrandProvider {
    client: Arc<FcpClient>,
    inner: HttpBrandProvider,
}

impl FcpBrandProvider {
    pub fn new(client: Arc<FcpClient>) -> Result<Self, AnvilError> {
        Ok(Self {
            client,
            inner: HttpBrandProvider::new()?,
        })
    }

    async fn signed(&self, mut req: BrandRequest) -> Result<BrandRequest, AnvilError> {
        let token = self
            .client
            .bearer()
            .await
            .map_err(|e| AnvilError::AuthRejected(e.to_string()))?;
        req.credential = BrandCredential::Bearer(token);
        Ok(req)
    }
}

#[async_trait]
impl BrandProvider for FcpBrandProvider {
    async fn fetch(&self, req: BrandRequest) -> Result<BrandFetchOutcome, AnvilError> {
        match self.inner.fetch(self.signed(req.clone()).await?).await {
            // Revoked or expired underneath us: once more after a refresh.
            Err(AnvilError::AuthRejected(_)) => {
                self.client
                    .refresh()
                    .await
                    .map_err(|e| AnvilError::AuthRejected(e.to_string()))?;
                self.inner.fetch(self.signed(req).await?).await
            }
            other => other,
        }
    }
}

/// Branding for an account signed in to FCP: the provider above, and a
/// filesystem cache (`cache_dir`, or the platform's).
pub fn brand_config(
    client: Arc<FcpClient>,
    cache_dir: Option<std::path::PathBuf>,
) -> Result<BrandConfig, AnvilError> {
    let cache: Box<dyn BrandCache> = match cache_dir {
        Some(dir) => Box::new(FsBrandCache::at(dir)),
        None => Box::new(FsBrandCache::new()?),
    };
    Ok(BrandConfig {
        provider: Some(Box::new(FcpBrandProvider::new(client)?)),
        cache: Some(cache),
        credential: BrandCredential::None,
    })
}
