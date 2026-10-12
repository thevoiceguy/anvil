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
        // Windows names the machine in COMPUTERNAME; Unix in HOSTNAME or
        // /etc/hostname.
        let host = std::env::var("HOSTNAME")
            .ok()
            .or_else(|| std::env::var("COMPUTERNAME").ok())
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
/// user alone reads ([`FileTokenStore`]: owner-only on Unix, sealed with
/// DPAPI on Windows); a mobile host supplies its Keychain or Keystore.
pub trait TokenStore: Send + Sync {
    fn load(&self) -> Result<Option<Session>, FcpError>;
    fn save(&self, session: &Session) -> Result<(), FcpError>;
    fn clear(&self) -> Result<(), FcpError>;
}

/// What a session file sealed with DPAPI begins with; a file without it is
/// the JSON itself (Unix, or Windows before sealing).
#[cfg(windows)]
const SEALED: &[u8] = b"anvil-dpapi-1\n";

/// A session in a JSON file, readable by its owner only: mode 0600 on Unix,
/// sealed for the user with DPAPI on Windows (an older clear file is read
/// and sealed).
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
        let store =
            |e: &dyn std::fmt::Display| FcpError::Store(format!("{}: {e}", self.path.display()));
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(store(&e)),
        };
        #[cfg(windows)]
        if let Some(sealed) = bytes.strip_prefix(SEALED) {
            let json = crate::dpapi::unseal(sealed).map_err(|e| store(&e))?;
            return serde_json::from_slice(&json)
                .map(Some)
                .map_err(|e| store(&e));
        }
        let session: Session = serde_json::from_slice(&bytes).map_err(|e| store(&e))?;
        // Kept in the clear by an older version: sealed from now on.
        #[cfg(windows)]
        if let Err(e) = self.save(&session) {
            tracing::warn!(%e, "the session was not sealed");
        }
        Ok(Some(session))
    }

    fn save(&self, session: &Session) -> Result<(), FcpError> {
        let store = |e: std::io::Error| FcpError::Store(format!("{}: {e}", self.path.display()));
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(store)?;
        }
        let json =
            serde_json::to_vec_pretty(session).map_err(|e| FcpError::Store(e.to_string()))?;
        #[cfg(windows)]
        let json = [SEALED, &crate::dpapi::seal(&json).map_err(store)?].concat();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session {
            server: Server {
                public_url: "https://pbx.example.com".into(),
                api_url: "https://pbx.example.com/api/v1".into(),
                authorize_url: "https://pbx.example.com/auth/app/authorize".into(),
                token_url: "https://pbx.example.com/auth/app/token".into(),
            },
            access_token: "access-secret".into(),
            access_expires_at: 0,
            refresh_token: "refresh-secret".into(),
            session_id: "s1".into(),
            username: "ann".into(),
            device: Some(DeviceCredentials {
                id: "d1".into(),
                name: "Anvil".into(),
                sip_username: "ann-anvil".into(),
                sip_password: "sip-secret".into(),
                realm: "pbx.example.com".into(),
            }),
            client: AppClient::this_machine("install"),
        }
    }

    fn dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "anvil-store-{}-{}",
            std::process::id(),
            AppClient::new_install_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_kept_session_comes_back_and_no_secret_is_in_the_clear_where_it_can_be() {
        let dir = dir();
        let store = FileTokenStore::new(dir.join("session.json"));
        store.save(&session()).unwrap();
        assert_eq!(store.load().unwrap(), Some(session()));
        let raw = String::from_utf8_lossy(&std::fs::read(store.path()).unwrap()).into_owned();
        if cfg!(windows) {
            for secret in ["access-secret", "refresh-secret", "sip-secret"] {
                assert!(!raw.contains(secret), "{secret} is in the clear");
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_session_kept_in_the_clear_is_read_and_sealed() {
        let dir = dir();
        let store = FileTokenStore::new(dir.join("session.json"));
        std::fs::write(store.path(), serde_json::to_vec(&session()).unwrap()).unwrap();
        assert_eq!(store.load().unwrap(), Some(session()));
        let raw = String::from_utf8_lossy(&std::fs::read(store.path()).unwrap()).into_owned();
        assert_eq!(
            raw.contains("refresh-secret"),
            !cfg!(windows),
            "sealed on Windows, left as it is elsewhere"
        );
        assert_eq!(store.load().unwrap(), Some(session()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn this_machine_is_named() {
        if std::env::var_os("HOSTNAME").is_some() || std::env::var_os("COMPUTERNAME").is_some() {
            assert_ne!(
                AppClient::this_machine("i").client_name,
                "Anvil on this computer"
            );
        }
    }
}
