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

/// An identity provider a tenant signs in through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SsoProvider {
    pub id: String,
    pub name: String,
}

/// How a person signs in, as FCP's `POST /auth/discover` says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInMethods {
    /// A password is taken.
    pub password: bool,
    /// The tenant has identity providers to sign in through (the browser).
    pub sso: bool,
    pub tenant_name: Option<String>,
    pub providers: Vec<SsoProvider>,
}

impl SignInMethods {
    /// From the discover route's answer (`{methods, tenant_name, providers}`).
    pub fn from_answer(answer: &serde_json::Value) -> Self {
        let methods: Vec<&str> = answer["methods"]
            .as_array()
            .map(|m| m.iter().filter_map(|m| m.as_str()).collect())
            .unwrap_or_default();
        let providers: Vec<SsoProvider> = answer["providers"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|p| serde_json::from_value(p.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            password: methods.contains(&"password"),
            sso: methods.contains(&"sso") || !providers.is_empty(),
            tenant_name: answer["tenant_name"].as_str().map(String::from),
            providers,
        }
    }
}

/// What to ask FCP's discover route about: an email address, since FCP
/// finds a tenant by its email domain — the username when it is one, else
/// the server field when that is one, else the username.
pub fn discovery_login(place: &str, username: &str) -> String {
    let (place, username) = (place.trim(), username.trim());
    if username.contains('@') {
        return username.to_string();
    }
    if place.contains('@') && !place.contains("://") {
        return place.to_string();
    }
    username.to_string()
}

/// How `login` signs in on `server`. `None` when the server has no
/// discover route (an FCP from before it): the caller offers both ways.
pub async fn sign_in_methods(
    server: &Server,
    login: &str,
) -> Result<Option<SignInMethods>, FcpError> {
    let response = http()
        .post(format!("{}/auth/discover", server.api_url))
        .json(&serde_json::json!({ "login": login.trim() }))
        .send()
        .await?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::METHOD_NOT_ALLOWED
    {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(refused(response).await);
    }
    let answer: serde_json::Value = response
        .json()
        .await
        .map_err(|e| FcpError::Invalid(format!("/auth/discover: {e}")))?;
    Ok(Some(SignInMethods::from_answer(&answer)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_email_is_what_fcp_is_asked_about() {
        assert_eq!(
            discovery_login("pbx.example.com", "ann@example.com"),
            "ann@example.com"
        );
        assert_eq!(discovery_login("ann@example.com", "ann"), "ann@example.com");
        assert_eq!(discovery_login("https://pbx.example.com", "ann"), "ann");
        assert_eq!(discovery_login(" pbx ", " ann "), "ann");
    }

    #[test]
    fn the_ways_in_are_read() {
        let m = SignInMethods::from_answer(&serde_json::json!({
            "tenant_id": "tenant-1", "tenant_name": "Acme",
            "methods": ["sso"],
            "providers": [{"id": "p1", "name": "Okta"}]
        }));
        assert!(!m.password && m.sso);
        assert_eq!(m.tenant_name.as_deref(), Some("Acme"));
        assert_eq!(m.providers[0].name, "Okta");

        let m = SignInMethods::from_answer(&serde_json::json!({"methods": ["password"]}));
        assert!(m.password && !m.sso);
        assert!(m.providers.is_empty());
    }
}
