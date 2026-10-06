//! HTTP-based `BrandProvider` using reqwest (`docs/BRANDING.md` §3–§6).

use std::time::Duration;

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use anvil_core::{
    AnvilError, BrandAsset, BrandCredential, BrandFetchOutcome, BrandProfile, BrandProvider,
    BrandRequest,
};

/// The largest asset fetched (`docs/BRANDING.md` §5).
const MAX_ASSET_BYTES: usize = 5 * 1024 * 1024;

/// Fetches brand profiles over HTTPS from an FCP provisioning endpoint.
///
/// The provider is stateless — one instance can serve multiple accounts —
/// but the reqwest `Client` is kept for connection pooling and HTTP/2.
pub struct HttpBrandProvider {
    client: reqwest::Client,
}

impl HttpBrandProvider {
    /// Create a provider with default HTTP settings (rustls, HTTP/2 allowed,
    /// 10s connect timeout, 30s total timeout).
    pub fn new() -> Result<Self, AnvilError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(1))
            .user_agent(concat!("Anvil/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| AnvilError::Internal(format!("HTTP client: {e}")))?;
        Ok(Self { client })
    }

    /// Fetch one asset and verify it against its hash; `None` when it cannot
    /// be had or is not what the profile says (the rest of the profile
    /// stands).
    async fn asset(&self, asset: &BrandAsset) -> Option<Vec<u8>> {
        if !allowed(&asset.url) {
            tracing::warn!(url = %asset.url, "brand asset over plain HTTP; dropped");
            return None;
        }
        let response = self.client.get(&asset.url).send().await.ok()?;
        if !response.status().is_success() {
            tracing::warn!(url = %asset.url, status = %response.status(), "brand asset not fetched");
            return None;
        }
        if response
            .content_length()
            .is_some_and(|l| l as usize > MAX_ASSET_BYTES)
        {
            return None;
        }
        let bytes = response.bytes().await.ok()?;
        if bytes.len() > MAX_ASSET_BYTES {
            return None;
        }
        let got = hex::encode(Sha256::digest(&bytes));
        if !got.eq_ignore_ascii_case(&asset.sha256) {
            tracing::warn!(url = %asset.url, "brand asset does not match its hash; dropped");
            return None;
        }
        Some(bytes.to_vec())
    }

    async fn fill(&self, asset: &mut Option<BrandAsset>) {
        if let Some(a) = asset {
            match self.asset(a).await {
                Some(bytes) => a.bytes = bytes,
                None => *asset = None,
            }
        }
    }
}

/// HTTPS, or plain HTTP to this machine only (a developer's FCP): the
/// contract refuses `http://` otherwise (`docs/BRANDING.md` §8).
fn allowed(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url) else {
        return false;
    };
    match u.scheme() {
        "https" => true,
        "http" => match u.host() {
            Some(url::Host::Domain(d)) => d == "localhost",
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        },
        _ => false,
    }
}

#[async_trait]
impl BrandProvider for HttpBrandProvider {
    async fn fetch(&self, req: BrandRequest) -> Result<BrandFetchOutcome, AnvilError> {
        let base = req.provisioning_url.trim_end_matches('/');
        if !allowed(base) {
            return Err(AnvilError::Config(format!(
                "the provisioning URL {base} is not https://"
            )));
        }
        let mut request = self
            .client
            .get(format!("{base}/brand/v1"))
            .query(&[("aor", req.aor.as_str())])
            .header(reqwest::header::ACCEPT, "application/json");
        request = match &req.credential {
            BrandCredential::Bearer(token) => request.bearer_auth(token),
            BrandCredential::Basic { username, password } => {
                request.basic_auth(username, Some(password))
            }
            BrandCredential::None => request,
        };
        if let Some(etag) = &req.if_none_match {
            request = request.header(reqwest::header::IF_NONE_MATCH, format!("\"{etag}\""));
        }
        let response = request
            .send()
            .await
            .map_err(|e| AnvilError::Transport(format!("brand fetch: {e}")))?;
        match response.status().as_u16() {
            304 => return Ok(BrandFetchOutcome::NotModified),
            404 => return Ok(BrandFetchOutcome::NoBrand),
            401 | 403 => {
                return Err(AnvilError::AuthRejected(format!(
                    "the brand endpoint answered {}",
                    response.status()
                )))
            }
            s if !(200..300).contains(&s) => {
                return Err(AnvilError::Transport(format!(
                    "the brand endpoint answered {}",
                    response.status()
                )))
            }
            _ => {}
        }
        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(|v| {
                v.trim()
                    .trim_start_matches("W/")
                    .trim_matches('"')
                    .to_string()
            });
        let mut profile: BrandProfile = response
            .json()
            .await
            .map_err(|e| AnvilError::Internal(format!("brand profile: {e}")))?;
        if profile.schema_version != 1 {
            return Err(AnvilError::Internal(format!(
                "brand profile version {} is not one this Anvil reads",
                profile.schema_version
            )));
        }
        if let Some(etag) = etag {
            profile.etag = etag;
        }
        self.fill(&mut profile.logo).await;
        self.fill(&mut profile.logo_dark).await;
        self.fill(&mut profile.icon).await;
        let mut ringtones = Vec::new();
        for mut ringtone in std::mem::take(&mut profile.ringtones) {
            if let Some(bytes) = self.asset(&ringtone.asset).await {
                ringtone.asset.bytes = bytes;
                ringtones.push(ringtone);
            }
        }
        profile.ringtones = ringtones;
        profile.fetched_at = Some(std::time::SystemTime::now());
        Ok(BrandFetchOutcome::Updated(Box::new(profile)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_or_this_machine() {
        assert!(allowed("https://pbx.example.com"));
        assert!(allowed("http://127.0.0.1:18080"));
        assert!(allowed("http://localhost:8080/x"));
        assert!(allowed("http://[::1]:8080"));
        assert!(!allowed("http://pbx.example.com"));
        assert!(!allowed("ftp://pbx.example.com"));
        assert!(!allowed("not a url"));
    }
}
