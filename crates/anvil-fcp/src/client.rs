//! The app session in use: kept fresh, and the softphone's settings.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{http, refused, FcpError, Session, TokenStore};

/// The SIP account the app registers with (`GET /me/softphone`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoftphoneAccount {
    pub aor: String,
    pub display_name: Option<String>,
    pub domain: String,
    pub registrar: String,
    pub outbound_proxy: Option<String>,
    pub transport: String,
    pub realm: String,
    pub sip_username: String,
    pub register_expires: u32,
}

/// The media the app offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoftphoneMedia {
    pub codecs: Vec<String>,
    pub srtp: String,
    pub dtmf: String,
}

/// Everything `GET /me/softphone` says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoftphoneConfig {
    pub account: SoftphoneAccount,
    pub media: SoftphoneMedia,
    pub username: String,
    pub extension: Option<String>,
    #[serde(default)]
    pub feature_codes: std::collections::BTreeMap<String, String>,
    pub api_url: Option<String>,
    pub device_id: String,
    /// The user may record a call they are on (FCP v0.25.466); absent from
    /// an older FCP, which has no such route.
    #[serde(default)]
    pub may_record: bool,
    /// The user is listed in the tenant's directory (FCP v0.25.471; an
    /// older FCP lists everyone).
    #[serde(default = "listed")]
    pub directory_listed: bool,
    /// The tenant lets its users take themselves out of the directory
    /// ([`FcpClient::set_directory_listed`]).
    #[serde(default)]
    pub may_change_directory_listing: bool,
    /// The app's lines, its own first (FCP v0.25.487, `docs/DEVICE_LINES.md`);
    /// empty from an older FCP, which has the account alone.
    #[serde(default)]
    pub lines: Vec<SoftphoneLine>,
    /// The tenant lets its users name their lines
    /// ([`FcpClient::set_line_label`]).
    #[serde(default)]
    pub may_rename_lines: bool,
    /// Whether the app may capture calls (FCP v0.25.491,
    /// `docs/CALL_DIAGNOSTICS.md` §3); an older FCP says nothing, and the
    /// app may not.
    #[serde(default)]
    pub capture: SoftphoneCapture,
}

/// Whether the app may capture calls, until when, and how large a capture
/// may be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoftphoneCapture {
    pub allowed: bool,
    /// RFC 3339, while allowed.
    #[serde(default)]
    pub until: Option<String>,
    #[serde(default = "capture_max_mb")]
    pub max_mb: u32,
}

fn capture_max_mb() -> u32 {
    50
}

impl Default for SoftphoneCapture {
    fn default() -> Self {
        Self {
            allowed: false,
            until: None,
            max_mb: capture_max_mb(),
        }
    }
}

/// A line on the app: an account it registers with its own credentials and
/// takes calls on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoftphoneLine {
    /// 1 for the first, which is the user's own.
    pub index: u32,
    /// `own`, `extension`, `shared` or `on_behalf`.
    pub kind: String,
    /// The line's key: what `X-FCP-Line` names on a call for it, and what
    /// a name for it is set under.
    pub line: String,
    /// What the app registers it as.
    pub aor: String,
    pub label: String,
    pub default_label: String,
    #[serde(default)]
    pub shared: bool,
    #[serde(default)]
    pub appearances: u32,
    /// A shared line whose active calls the app may join (FCP v0.25.489).
    #[serde(default)]
    pub barge_in: bool,
    /// Whether calls to the line ring the app.
    #[serde(default = "listed")]
    pub ring: bool,
}

fn listed() -> bool {
    true
}

impl SoftphoneConfig {
    /// A feature code by FCP's name for it (`park`, `retrieve`,
    /// `group_pickup`, `directed_pickup`, …).
    pub fn feature_code(&self, name: &str) -> Option<&str> {
        self.feature_codes.get(name).map(String::as_str)
    }

    /// The media FCP says the app offers: its codecs in FCP's order (the
    /// ones Anvil drives) and SRTP — `optional` over TLS, `off` otherwise.
    pub fn media_config(&self) -> anvil_core::MediaConfig {
        use anvil_core::config::{Codec, SrtpMode};
        let mut codecs = Vec::new();
        for name in &self.media.codecs {
            let codec = match name.to_ascii_lowercase().as_str() {
                "opus" => Some(Codec::Opus),
                "g722" => Some(Codec::G722),
                "pcmu" => Some(Codec::Pcmu),
                "pcma" => Some(Codec::Pcma),
                _ => None,
            };
            if let Some(c) = codec.filter(|c| !codecs.contains(c)) {
                codecs.push(c);
            }
        }
        let defaults = anvil_core::MediaConfig::default();
        anvil_core::MediaConfig {
            codecs: if codecs.is_empty() {
                defaults.codecs.clone()
            } else {
                codecs
            },
            srtp: match self.media.srtp.as_str() {
                "required" => SrtpMode::Required,
                "optional" => SrtpMode::Optional,
                _ => SrtpMode::Off,
            },
            ..defaults
        }
    }

    /// A feature code as an address in the account's domain: what to
    /// transfer a call to (park) or to call (retrieve, pickup).
    pub fn feature_uri(&self, name: &str) -> Option<String> {
        self.feature_code(name)
            .map(|code| format!("sip:{code}@{}", self.account.domain))
    }
}

/// The user's calling settings (`/me/calling`, FCP's
/// `docs/ENTERPRISE_CALLING.md`): what a softphone shows and changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallingSettings {
    pub dnd: bool,
    pub call_waiting: bool,
    pub forward_all: Option<String>,
    pub forward_busy: Option<String>,
    pub forward_no_answer: Option<String>,
    pub forward_unreachable: Option<String>,
    pub no_answer_secs: Option<u32>,
}

/// A change to the calling settings: each field left out is kept. A
/// forward is cleared with an empty string.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallingUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dnd: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_waiting: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_all: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_busy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_no_answer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_unreachable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_answer_secs: Option<u32>,
}

/// FCP, through the app's session.
pub struct FcpClient {
    http: reqwest::Client,
    session: Mutex<Session>,
    store: Option<Arc<dyn TokenStore>>,
}

impl FcpClient {
    pub fn new(session: Session, store: Option<Arc<dyn TokenStore>>) -> Self {
        Self {
            http: http(),
            session: Mutex::new(session),
            store,
        }
    }

    /// The session as it is now.
    pub async fn session(&self) -> Session {
        self.session.lock().await.clone()
    }

    /// Refresh the session: new tokens, kept.
    pub async fn refresh(&self) -> Result<(), FcpError> {
        let mut session = self.session.lock().await;
        let response = self
            .http
            .post(&session.server.token_url)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", session.refresh_token.as_str()),
            ])
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(refused(response).await);
        }
        let tokens: serde_json::Value = response.json().await?;
        let renewed = Session::from_tokens(
            session.server.clone(),
            session.client.clone(),
            &tokens,
            session.device.clone(),
        )?;
        *session = renewed;
        if let Some(store) = &self.store {
            store.save(&session)?;
        }
        Ok(())
    }

    /// An access token good for a while, refreshing first if it is not.
    pub(crate) async fn bearer(&self) -> Result<String, FcpError> {
        if !self
            .session
            .lock()
            .await
            .access_fresh(Duration::from_secs(60))
        {
            self.refresh().await?;
        }
        Ok(self.session.lock().await.access_token.clone())
    }

    async fn api(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> Result<reqwest::Response, FcpError> {
        self.api_with(method, path, None).await
    }

    /// A request to FCP's API as the session, with a JSON body when given;
    /// a 401 is retried once after a refresh, any other refusal returned.
    pub(crate) async fn api_with(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<reqwest::Response, FcpError> {
        self.api_request(method, path, |request| match body {
            Some(body) => request.json(body),
            None => request,
        })
        .await
    }

    /// An API request with `body` as it is (a file).
    pub(crate) async fn api_bytes(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Vec<u8>,
    ) -> Result<reqwest::Response, FcpError> {
        self.api_request(method, path, |request| {
            request
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                .body(body.clone())
        })
        .await
    }

    async fn api_request(
        &self,
        method: reqwest::Method,
        path: &str,
        with: impl Fn(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, FcpError> {
        let api = self.session.lock().await.server.api_url.clone();
        let send = |token: String| {
            let request = self
                .http
                .request(method.clone(), format!("{api}{path}"))
                .bearer_auth(token);
            with(request).send()
        };
        let mut response = send(self.bearer().await?).await?;
        // Revoked or expired underneath us: once more after a refresh.
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.refresh().await?;
            response = send(self.bearer().await?).await?;
        }
        if !response.status().is_success() {
            return Err(refused(response).await);
        }
        Ok(response)
    }

    /// The softphone's settings.
    pub async fn softphone(&self) -> Result<SoftphoneConfig, FcpError> {
        let response = self.api(reqwest::Method::GET, "/me/softphone").await?;
        response
            .json()
            .await
            .map_err(|e| FcpError::Invalid(format!("/me/softphone: {e}")))
    }

    /// The user's calling settings.
    pub async fn calling(&self) -> Result<CallingSettings, FcpError> {
        let response = self.api(reqwest::Method::GET, "/me/calling").await?;
        response
            .json()
            .await
            .map_err(|e| FcpError::Invalid(format!("/me/calling: {e}")))
    }

    /// Change the calling settings; what they are afterwards.
    pub async fn set_calling(&self, update: &CallingUpdate) -> Result<CallingSettings, FcpError> {
        let body = serde_json::to_value(update)
            .map_err(|e| FcpError::Invalid(format!("calling update: {e}")))?;
        let response = self
            .api_with(reqwest::Method::PUT, "/me/calling", Some(&body))
            .await?;
        response
            .json()
            .await
            .map_err(|e| FcpError::Invalid(format!("/me/calling: {e}")))
    }

    /// A new SIP password for the device, kept with the session.
    pub async fn rotate_password(&self) -> Result<(), FcpError> {
        let response = self
            .api(reqwest::Method::POST, "/me/softphone/rotate-password")
            .await?;
        let body: serde_json::Value = response.json().await?;
        let mut session = self.session.lock().await;
        if let (Some(device), Some(password)) =
            (session.device.as_mut(), body["sip_password"].as_str())
        {
            device.sip_password = password.to_string();
        }
        if let Some(store) = &self.store {
            store.save(&session)?;
        }
        Ok(())
    }

    /// Sign the app out: its session ends at FCP and is forgotten here.
    pub async fn sign_out(&self) -> Result<(), FcpError> {
        let session = self.session.lock().await.clone();
        let _ = self
            .http
            .post(format!("{}/auth/app/revoke", session.server.api_url))
            .json(&serde_json::json!({ "token": session.refresh_token }))
            .send()
            .await;
        if let Some(store) = &self.store {
            store.clear()?;
        }
        Ok(())
    }

    /// The account `anvil-core` registers with, from the settings and the
    /// device's SIP password.
    pub async fn account_config(
        &self,
        config: &SoftphoneConfig,
    ) -> Result<anvil_core::AccountConfig, FcpError> {
        let session = self.session.lock().await.clone();
        let device = session.device.ok_or_else(|| {
            FcpError::Invalid("the session has no device: sign in again".to_string())
        })?;
        let transport = match config.account.transport.as_str() {
            "tls" => anvil_core::Transport::Tls,
            "tcp" => anvil_core::Transport::Tcp,
            _ => anvil_core::Transport::Udp,
        };
        Ok(anvil_core::AccountConfig {
            aor: config.account.aor.clone(),
            registrar: config.account.registrar.clone(),
            username: config.account.sip_username.clone(),
            password: device.sip_password,
            transport,
            outbound_proxy: config.account.outbound_proxy.clone(),
            stun: None,
            register_expires: Duration::from_secs(u64::from(config.account.register_expires)),
            user_agent: format!("Anvil/{}", env!("CARGO_PKG_VERSION")),
            bind_addr: None,
            tls_extra_ca_pem: None,
            provisioning_url: Some(session.server.public_url),
            instance_id: instance_urn(&session.client.install_id),
            push: None,
        })
    }
}

/// The install's RFC 5626 instance, from its install id (32 hex digits, a
/// UUID's): `<urn:uuid:…>`. The same on every start, so FCP replaces the
/// install's binding wherever it registers from.
pub fn instance_urn(install_id: &str) -> Option<String> {
    let id = install_id.trim().to_ascii_lowercase();
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!(
        "<urn:uuid:{}-{}-{}-{}-{}>",
        &id[..8],
        &id[8..12],
        &id[12..16],
        &id[16..20],
        &id[20..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(codecs: &[&str], srtp: &str) -> SoftphoneConfig {
        serde_json::from_value(serde_json::json!({
            "account": {
                "aor": "sip:a@x", "registrar": "sip:x", "transport": "tls", "outbound_proxy": null,
                "domain": "x", "realm": "x", "sip_username": "a", "register_expires": 3600
            },
            "media": {"codecs": codecs, "srtp": srtp, "dtmf": "rfc2833"},
            "username": "a", "extension": null, "feature_codes": {}, "api_url": null,
            "device_id": "d"
        }))
        .unwrap()
    }

    #[test]
    fn an_install_is_one_instance() {
        assert_eq!(
            instance_urn("0123456789abcdef0123456789ABCDEF").as_deref(),
            Some("<urn:uuid:01234567-89ab-cdef-0123-456789abcdef>")
        );
        assert_eq!(instance_urn("not-an-id"), None);
    }

    #[test]
    fn media_follows_what_fcp_says() {
        use anvil_core::config::{Codec, SrtpMode};
        let m = config(&["PCMU", "opus", "speex"], "optional").media_config();
        assert_eq!(m.codecs, vec![Codec::Pcmu, Codec::Opus]);
        assert_eq!(m.srtp, SrtpMode::Optional);
        let m = config(&[], "off").media_config();
        assert_eq!(m.srtp, SrtpMode::Off);
        assert!(
            !m.codecs.is_empty(),
            "Anvil's own codecs when FCP names none"
        );
    }
}
