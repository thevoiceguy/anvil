//! Anvil as FCP's client (`docs/SOFTPHONE.md` in thevoiceguy/fcp):
//!
//! - [`discover`] finds FCP from a server address or an email address, and
//!   [`sign_in_methods`] asks it how a person signs in (password, single
//!   sign-on);
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

// Unsafe only where Windows' API needs it, in one module.
#![deny(unsafe_code)]

mod brand;
mod client;
mod discovery;
#[cfg(windows)]
#[allow(unsafe_code)]
mod dpapi;
mod me;
mod remember;
mod session;
mod signin;

pub use brand::{brand_config, FcpBrandProvider};
pub use client::{
    CallingSettings, CallingUpdate, FcpClient, SoftphoneAccount, SoftphoneConfig, SoftphoneMedia,
};
pub use discovery::{
    discover, discovery_login, sign_in_methods, Server, SignInMethods, SsoProvider,
};
pub use me::{
    CallQuery, CallRecord, ContactFields, ContactImport, ContactImportSkip, ContactNumber,
    DirectoryEntry, MailboxStats, Page, PersonalContact, UserEvent, UserEvents, VoicemailMessage,
};
pub use remember::LastSignIn;
pub use session::{AppClient, DeviceCredentials, FileTokenStore, Session, TokenStore};
pub use signin::{
    begin_password_sign_in, password_sign_in, BrowserSignIn, PasswordStep, SecondFactor,
};

/// What can go wrong talking to FCP.
#[derive(Debug, thiserror::Error)]
pub enum FcpError {
    #[error("FCP could not be reached: {0}")]
    Unreachable(String),
    #[error("FCP refused: {status} {message}")]
    Refused { status: u16, message: String },
    #[error("signing in needs a one-time code (TOTP)")]
    TotpRequired,
    /// A wrong password or one-time code, in FCP's words.
    #[error("{0}")]
    Rejected(String),
    #[error("your organisation requires a one-time code you have not set up: sign in with your browser to set it up")]
    SecondFactorSetup,
    #[error("your password has expired: sign in with your browser to change it")]
    PasswordExpired,
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
