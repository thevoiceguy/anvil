//! Anvil as FCP's client (`docs/SOFTPHONE.md` in thevoiceguy/fcp):
//!
//! - [`discover`] finds FCP from a server address or an email address;
//! - [`BrowserSignIn`] signs a person in through the system browser
//!   (OAuth 2.0 for native apps with PKCE: a loopback listener catches the
//!   code), and [`password_sign_in`] does it without a browser;
//! - a sign-in answers a [`Session`]: the app session's tokens and the
//!   install's device with its SIP account, kept by a [`TokenStore`];
//! - [`FcpClient`] keeps the session fresh and reads the softphone's
//!   settings (`/me/softphone`), which [`FcpClient::account_config`] turns
//!   into an [`anvil_core::AccountConfig`];
//! - and the user's own data through it: call history, the directory,
//!   voicemail and live events ([`FcpClient::events`]).
//!
//! `anvil-core` knows SIP and media, not FCP; this crate is FCP's.

#![forbid(unsafe_code)]

mod brand;
mod client;
mod discovery;
mod me;
mod session;
mod signin;

pub use brand::{brand_config, FcpBrandProvider};
pub use client::{
    CallingSettings, CallingUpdate, FcpClient, SoftphoneAccount, SoftphoneConfig, SoftphoneMedia,
};
pub use discovery::{discover, Server};
pub use me::{
    CallQuery, CallRecord, DirectoryEntry, MailboxStats, Page, UserEvent, UserEvents,
    VoicemailMessage,
};
pub use session::{AppClient, DeviceCredentials, FileTokenStore, Session, TokenStore};
pub use signin::{password_sign_in, BrowserSignIn};

/// What can go wrong talking to FCP.
#[derive(Debug, thiserror::Error)]
pub enum FcpError {
    #[error("FCP could not be reached: {0}")]
    Unreachable(String),
    #[error("FCP refused: {status} {message}")]
    Refused { status: u16, message: String },
    #[error("signing in needs a one-time code (TOTP)")]
    TotpRequired,
    #[error("the session has ended: sign in again")]
    SignedOut,
    #[error("the browser sign-in failed: {0}")]
    BrowserSignIn(String),
    #[error("{0}")]
    Invalid(String),
    #[error("the token store: {0}")]
    Store(String),
}

impl From<reqwest::Error> for FcpError {
    fn from(e: reqwest::Error) -> Self {
        FcpError::Unreachable(e.to_string())
    }
}

/// A reqwest client for FCP: rustls, no redirects followed (an app's
/// sign-in reads them), a sensible timeout.
pub(crate) fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(20))
        .user_agent(concat!("Anvil/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("a reqwest client builds")
}

/// FCP's error body (`{error, code, details}`) as a refusal.
pub(crate) async fn refused(response: reqwest::Response) -> FcpError {
    let status = response.status().as_u16();
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    let message = body["details"]
        .as_str()
        .or_else(|| body["error"].as_str())
        .unwrap_or_default()
        .to_string();
    if status == 401 {
        return FcpError::SignedOut;
    }
    FcpError::Refused { status, message }
}
