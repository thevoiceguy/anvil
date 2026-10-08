//! What the phone keeps on this device: favourites and the audio devices
//! chosen. FCP keeps everything else.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LocalSettings {
    /// People by [`Person::key`](crate::Person::key).
    #[serde(default)]
    pub favourites: BTreeSet<String>,
    #[serde(default)]
    pub audio_input: Option<String>,
    #[serde(default)]
    pub audio_output: Option<String>,
}

impl LocalSettings {
    /// The settings at `path`; the defaults when there are none yet or they
    /// cannot be read (which is logged: the phone starts regardless).
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!(%e, path = %path.display(), "settings not read");
                Self::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                tracing::warn!(%e, path = %path.display(), "settings not read");
                Self::default()
            }
        }
    }

    /// Write them to `path`, whole: a temporary file renamed over it.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_come_back_as_saved() {
        let dir = std::env::temp_dir().join(format!(
            "anvil-settings-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("settings.json");
        assert_eq!(LocalSettings::load(&path), LocalSettings::default());
        let saved = LocalSettings {
            favourites: ["alice".to_string()].into(),
            audio_input: Some("USB Headset".into()),
            audio_output: None,
        };
        saved.save(&path).unwrap();
        assert_eq!(LocalSettings::load(&path), saved);
        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(LocalSettings::load(&path), LocalSettings::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
