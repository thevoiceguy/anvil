//! Finding FCP (`docs/SOFTPHONE.md` §2.6): `/.well-known/fcp-provisioning`
//! on the server, or on an email address's domain.

use serde::{Deserialize, Serialize};

use crate::{http, refused, FcpError};

/// Where an FCP is, as its well-known document says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    pub public_url: String,
    pub api_url: String,
    pub authorize_url: String,
    pub token_url: String,
}

/// Find FCP from `place`: a server address (`https://pbx.example.com`, or
/// `http://…` for a lab) or an email address, whose domain is asked.
pub async fn discover(place: &str) -> Result<Server, FcpError> {
    let place = place.trim();
    let base = if let Some((_, domain)) = place.split_once('@').filter(|_| !place.contains("://")) {
        format!("https://{domain}")
    } else if place.contains("://") {
        place.trim_end_matches('/').to_string()
    } else {
        format!("https://{}", place.trim_end_matches('/'))
    };
    let url = format!("{base}/.well-known/fcp-provisioning");
    let response = http().get(&url).send().await?;
    if !response.status().is_success() {
        return Err(refused(response).await);
    }
    #[derive(Deserialize)]
    struct WellKnown {
        public_url: Option<String>,
        api_url: Option<String>,
        authorize_url: Option<String>,
        token_url: Option<String>,
    }
    let known: WellKnown = response
        .json()
        .await
        .map_err(|e| FcpError::Invalid(format!("{url}: {e}")))?;
    // A document without addresses (no public URL configured) names the
    // server it was fetched from.
    let public_url = known.public_url.unwrap_or_else(|| base.clone());
    Ok(Server {
        api_url: known
            .api_url
            .unwrap_or_else(|| format!("{public_url}/api/v1")),
        authorize_url: known
            .authorize_url
            .unwrap_or_else(|| format!("{public_url}/api/v1/auth/app/authorize")),
        token_url: known
            .token_url
            .unwrap_or_else(|| format!("{public_url}/api/v1/auth/app/token")),
        public_url,
    })
}

impl Server {
    /// A server at `base` without asking it (tests, a pre-configured install).
    pub fn at(base: &str) -> Self {
        let base = base.trim_end_matches('/').to_string();
        Self {
            api_url: format!("{base}/api/v1"),
            authorize_url: format!("{base}/api/v1/auth/app/authorize"),
            token_url: format!("{base}/api/v1/auth/app/token"),
            public_url: base,
        }
    }
}
