//! HTTP-based `BrandProvider` using reqwest. Phase 2 target — stub today.

use async_trait::async_trait;

use anvil_core::{
    AnvilError, BrandFetchOutcome, BrandProvider, BrandRequest,
};

/// Fetches brand profiles over HTTPS from an FCP provisioning endpoint.
///
/// The provider is stateless — one instance can serve multiple accounts —
/// but the reqwest `Client` is kept for connection pooling and HTTP/2.
pub struct HttpBrandProvider {
    _private: (),
}

impl HttpBrandProvider {
    /// Create a provider with default HTTP settings (rustls, HTTP/2 allowed,
    /// 10s connect timeout, 30s total timeout).
    pub fn new() -> Result<Self, AnvilError> {
        Ok(Self { _private: () })
    }
}

#[async_trait]
impl BrandProvider for HttpBrandProvider {
    async fn fetch(&self, _req: BrandRequest) -> Result<BrandFetchOutcome, AnvilError> {
        Err(AnvilError::Internal("HttpBrandProvider::fetch not yet implemented".into()))
    }
}
