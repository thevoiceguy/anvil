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
}

impl SoftphoneConfig {
    /// A feature code by FCP's name for it (`park`, `retrieve`,
    /// `group_pickup`, `directed_pickup`, …).
    pub fn feature_code(&self, name: &str) -> Option<&str> {
        self.feature_codes.get(name).map(String::as_str)
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
#[derive(Debug, Clone, Default, Serialize)]
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
        let api = self.session.lock().await.server.api_url.clone();
        let mut response = self
            .http
            .request(method.clone(), format!("{api}{path}"))
            .bearer_auth(self.bearer().await?)
            .send()
            .await?;
        // Revoked or expired underneath us: once more after a refresh.
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.refresh().await?;
            response = self
                .http
                .request(method, format!("{api}{path}"))
                .bearer_auth(self.bearer().await?)
                .send()
                .await?;
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
        let api = self.session.lock().await.server.api_url.clone();
        let send = |token: String| {
            self.http
                .put(format!("{api}/me/calling"))
                .bearer_auth(token)
                .json(update)
                .send()
        };
        let mut response = send(self.bearer().await?).await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.refresh().await?;
            response = send(self.bearer().await?).await?;
        }
        if !response.status().is_success() {
            return Err(refused(response).await);
        }
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
        })
    }
}
