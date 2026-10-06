//! The app session and where it is kept.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{FcpError, Server};

/// The app, as it names itself to FCP: what the person sees in their list
/// of devices, and the install it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppClient {
    pub client_name: String,
    pub install_id: String,
    pub platform: Option<String>,
    pub version: Option<String>,
}

impl AppClient {
    /// This machine: its host name, a new install id, the OS.
    pub fn this_machine(install_id: impl Into<String>) -> Self {
        let host = std::env::var("HOSTNAME")
            .ok()
            .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
            .map(|h| h.trim().to_string())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "this computer".to_string());
        Self {
            client_name: format!("Anvil on {host}"),
            install_id: install_id.into(),
            platform: Some(std::env::consts::OS.to_string()),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }
    }

    /// A fresh install id.
    pub fn new_install_id() -> String {
        use rand::Rng;
        let bytes: [u8; 16] = rand::thread_rng().gen();
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// The install's device and its SIP account, as the sign-in gave them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCredentials {
    pub id: String,
    pub name: String,
    pub sip_username: String,
    pub sip_password: String,
    pub realm: String,
}

/// An app session: its tokens, the device, and the server it is on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub server: Server,
    pub access_token: String,
    /// Seconds since the epoch the access token stops.
    pub access_expires_at: u64,
    pub refresh_token: String,
    pub session_id: String,
    pub username: String,
    pub device: Option<DeviceCredentials>,
    pub client: AppClient,
}

impl Session {
    /// Whether the access token is still good for `margin`.
    pub fn access_fresh(&self, margin: Duration) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now + margin.as_secs() < self.access_expires_at
    }

    /// From FCP's token answer.
    pub(crate) fn from_tokens(
        server: Server,
        client: AppClient,
        tokens: &serde_json::Value,
        device: Option<DeviceCredentials>,
    ) -> Result<Self, FcpError> {
        let field = |name: &str| {
            tokens[name]
                .as_str()
                .map(String::from)
                .ok_or_else(|| FcpError::Invalid(format!("no {name} in FCP's answer")))
        };
        let expires_in = tokens["expires_in"].as_u64().unwrap_or(900);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let device = match &tokens["device"] {
            serde_json::Value::Null => device,
            d => Some(
                serde_json::from_value(d.clone())
                    .map_err(|e| FcpError::Invalid(format!("the device: {e}")))?,
            ),
        };
        Ok(Self {
            server,
            access_token: field("access_token")?,
            access_expires_at: now + expires_in,
            refresh_token: field("refresh_token")?,
            session_id: field("session_id")?,
            username: tokens["user"]["username"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            device,
            client,
        })
    }
}

/// Where a session is kept between runs. A desktop keeps it in a file the
/// user alone reads ([`FileTokenStore`]); a mobile host supplies its
/// Keychain or Keystore.
pub trait TokenStore: Send + Sync {
    fn load(&self) -> Result<Option<Session>, FcpError>;
    fn save(&self, session: &Session) -> Result<(), FcpError>;
    fn clear(&self) -> Result<(), FcpError>;
}

/// A session in a JSON file, readable by its owner only.
#[derive(Debug, Clone)]
pub struct FileTokenStore {
    path: PathBuf,
}

impl FileTokenStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl TokenStore for FileTokenStore {
    fn load(&self) -> Result<Option<Session>, FcpError> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| FcpError::Store(format!("{}: {e}", self.path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(FcpError::Store(format!("{}: {e}", self.path.display()))),
        }
    }

    fn save(&self, session: &Session) -> Result<(), FcpError> {
        let store = |e: std::io::Error| FcpError::Store(format!("{}: {e}", self.path.display()));
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(store)?;
        }
        let json =
            serde_json::to_vec_pretty(session).map_err(|e| FcpError::Store(e.to_string()))?;
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, json).map_err(store)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
                .map_err(store)?;
        }
        std::fs::rename(&tmp, &self.path).map_err(store)
    }

    fn clear(&self) -> Result<(), FcpError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(FcpError::Store(e.to_string())),
        }
    }
}
