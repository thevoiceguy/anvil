//! The last sign-in, remembered so the sign-in screen comes back filled in:
//! the server (or email address) and the username, never a password. Kept
//! in its own file beside the session's, which signing out deletes.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::FcpError;

/// Where and as whom the person last signed in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastSignIn {
    /// What they typed for the server: an address or an email address.
    pub place: String,
    pub username: String,
}

impl LastSignIn {
    /// The file beside a session file at `session_path`.
    pub fn path_beside(session_path: &Path) -> std::path::PathBuf {
        session_path.with_file_name("last_sign_in.json")
    }

    /// What was remembered at `path`; nothing when there is no file or it
    /// cannot be read (the person types it once more).
    pub fn load(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice::<Self>(&bytes)
            .ok()
            .filter(|l| !l.place.is_empty() || !l.username.is_empty())
    }

    /// Remember this sign-in at `path`.
    pub fn save(&self, path: &Path) -> Result<(), FcpError> {
        let store = |e: &dyn std::fmt::Display| FcpError::Store(format!("{}: {e}", path.display()));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| store(&e))?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(|e| store(&e))?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, json).map_err(|e| store(&e))?;
        std::fs::rename(&tmp, path).map_err(|e| store(&e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sign_in_is_remembered_beside_the_session_and_outlives_it() {
        let dir = std::env::temp_dir().join(format!(
            "anvil-last-{}-{}",
            std::process::id(),
            crate::AppClient::new_install_id()
        ));
        let session = dir.join("session.json");
        let path = LastSignIn::path_beside(&session);
        assert_eq!(path, dir.join("last_sign_in.json"));
        assert_eq!(LastSignIn::load(&path), None);

        let last = LastSignIn {
            place: "pbx.example.com".into(),
            username: "ann".into(),
        };
        last.save(&path).unwrap();
        assert_eq!(LastSignIn::load(&path), Some(last));
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("password"), "no password is kept: {raw}");

        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(LastSignIn::load(&path), None, "a spoilt file is forgotten");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
