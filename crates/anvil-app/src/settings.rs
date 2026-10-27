//! What the phone keeps on this device: favourites, the audio devices
//! chosen, the ringtone and the recent calls cleared. FCP keeps everything
//! else.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LocalSettings {
    /// People by [`Person::key`](crate::Person::key).
    #[serde(default)]
    pub favourites: BTreeSet<String>,
    #[serde(default)]
    pub audio_input: Option<String>,
    #[serde(default)]
    pub audio_output: Option<String>,
    /// The speaker to ring on; `None` is the system's default.
    #[serde(default)]
    pub ring_output: Option<String>,
    /// The ringtone by id (`ring`); `None` is the brand's, else Anvil's.
    #[serde(default)]
    pub ringtone: Option<String>,
    /// Echo cancellation; on unless turned off.
    #[serde(default = "on", skip_serializing_if = "is_on")]
    pub echo_cancellation: bool,
    /// Noise suppression; on unless turned off.
    #[serde(default = "on", skip_serializing_if = "is_on")]
    pub noise_suppression: bool,
    /// The microphone's volume in percent; 100 unless set.
    #[serde(default = "hundred", skip_serializing_if = "is_hundred")]
    pub microphone_volume: u16,
    /// The speaker's volume in percent; 100 unless set.
    #[serde(default = "hundred", skip_serializing_if = "is_hundred")]
    pub speaker_volume: u16,
    /// Recent calls cleared, by account: those that started at or before
    /// this time (RFC 3339) are not shown. FCP keeps their records.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub recents_cleared: BTreeMap<String, String>,
    /// Recent calls removed one at a time, by record id.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub hidden_recents: BTreeSet<String>,
    /// The contacts kept on this device, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contacts: Vec<crate::Contact>,
}

impl Default for LocalSettings {
    fn default() -> Self {
        Self {
            favourites: BTreeSet::new(),
            audio_input: None,
            audio_output: None,
            ring_output: None,
            ringtone: None,
            echo_cancellation: true,
            noise_suppression: true,
            microphone_volume: 100,
            speaker_volume: 100,
            recents_cleared: BTreeMap::new(),
            hidden_recents: BTreeSet::new(),
            contacts: Vec::new(),
        }
    }
}

fn on() -> bool {
    true
}

fn is_on(on: &bool) -> bool {
    *on
}

fn hundred() -> u16 {
    100
}

fn is_hundred(v: &u16) -> bool {
    *v == 100
}

impl LocalSettings {
    /// Whether `account`'s recent call is cleared or removed here.
    pub fn hides_recent(&self, account: &str, id: &str, started_at: &str) -> bool {
        self.hidden_recents.contains(id)
            || self
                .recents_cleared
                .get(account)
                .and_then(|cleared| Some(time(started_at)? <= time(cleared)?))
                .unwrap_or(false)
    }

    /// Clear `account`'s recent calls up to the newest of `started`.
    pub fn clear_recents<'a>(&mut self, account: &str, started: impl Iterator<Item = &'a str>) {
        let newest = started
            .filter_map(|s| Some((time(s)?, s)))
            .chain(
                self.recents_cleared
                    .get(account)
                    .and_then(|c| Some((time(c)?, c.as_str()))),
            )
            .max_by_key(|(t, _)| *t)
            .map(|(_, s)| s.to_string());
        if let Some(newest) = newest {
            self.recents_cleared.insert(account.to_string(), newest);
        }
    }

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

fn time(rfc3339: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(rfc3339).ok()
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
            ring_output: Some("Speakers".into()),
            ringtone: Some("classic".into()),
            echo_cancellation: false,
            noise_suppression: true,
            microphone_volume: 80,
            speaker_volume: 100,
            recents_cleared: [("sip:bob@d".to_string(), "2026-10-24T10:00:00Z".to_string())].into(),
            hidden_recents: ["cdr-1".to_string()].into(),
            contacts: vec![crate::Contact {
                id: "c1".into(),
                on_account: false,
                name: "Jo Smith".into(),
                company: Some("Acme".into()),
                numbers: vec![crate::ContactNumber {
                    label: "mobile".into(),
                    number: "0412345678".into(),
                }],
                email: None,
                notes: None,
            }],
        };
        saved.save(&path).unwrap();
        assert_eq!(LocalSettings::load(&path), saved);
        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(LocalSettings::load(&path), LocalSettings::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cleared_recents_stay_hidden_and_new_ones_show() {
        let mut local = LocalSettings::default();
        let bob = "sip:bob@d";
        assert!(!local.hides_recent(bob, "a", "2026-10-24T09:00:00Z"));
        local.clear_recents(
            bob,
            [
                "2026-10-24T09:00:00Z",
                "2026-10-24T10:00:00+02:00",
                "garbage",
            ]
            .into_iter(),
        );
        assert_eq!(local.recents_cleared[bob], "2026-10-24T09:00:00Z");
        assert!(local.hides_recent(bob, "a", "2026-10-24T08:59:59Z"));
        assert!(local.hides_recent(bob, "a", "2026-10-24T09:00:00Z"));
        assert!(!local.hides_recent(bob, "b", "2026-10-24T09:00:01Z"));
        assert!(!local.hides_recent("sip:carol@d", "c", "2026-10-24T08:00:00Z"));
        // Clearing nothing newer keeps the time it had.
        local.clear_recents(bob, std::iter::empty());
        assert_eq!(local.recents_cleared[bob], "2026-10-24T09:00:00Z");
        local.hidden_recents.insert("b".into());
        assert!(local.hides_recent(bob, "b", "2026-10-24T09:00:01Z"));
    }
}
