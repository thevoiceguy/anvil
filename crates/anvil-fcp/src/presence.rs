//! Presence a person sets (FCP's `docs/PRESENCE.md`, v0.25.473 on): the
//! status the user chose with a note and a time it clears at, their own
//! statuses, and what others see of them, with calls, do not disturb,
//! idleness and being signed out laid over it.

use serde::{Deserialize, Serialize};

use crate::me::json;
use crate::{FcpClient, FcpError};

/// What others see of a person.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Presence {
    /// `available`, `away`, `busy`, `dnd`, `ringing`, `on_call` or
    /// `offline`.
    pub state: String,
    /// Their own status's label ("WFH").
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// Away because their apps say they are idle.
    #[serde(default)]
    pub idle: bool,
}

/// One of the user's own statuses: a label on a state.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct OwnStatus {
    pub id: String,
    pub label: String,
    /// `available`, `away`, `busy` or `dnd`.
    pub state: String,
}

/// The user's presence (`/me/presence`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct MyPresence {
    /// What others see.
    pub presence: Presence,
    /// What they chose: `available`, `away`, `busy`, `dnd`, or one of
    /// `statuses`' ids.
    pub status: String,
    /// The chosen status's state.
    pub status_state: String,
    pub note: Option<String>,
    /// RFC 3339: when the chosen status clears back to available.
    pub until: Option<String>,
    pub statuses: Vec<OwnStatus>,
}

/// A change to the user's presence. `status` picks a status (with `note`
/// and `until`, cleared when left out); without it, `note` or `until`
/// given changes only that (`Some(None)` clears it).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PresenceUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle: Option<bool>,
}

#[derive(Deserialize)]
struct Statuses {
    data: Vec<OwnStatus>,
}

impl FcpClient {
    /// The user's presence; `None` from an FCP before presence a person
    /// sets (404).
    pub async fn presence(&self) -> Result<Option<MyPresence>, FcpError> {
        match self
            .api_with(reqwest::Method::GET, "/me/presence", None)
            .await
        {
            Ok(r) => json(r, "/me/presence").await.map(Some),
            Err(FcpError::Refused { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Change the user's presence; what it is afterwards.
    pub async fn set_presence(&self, update: &PresenceUpdate) -> Result<MyPresence, FcpError> {
        let body = serde_json::to_value(update)
            .map_err(|e| FcpError::Invalid(format!("a presence change: {e}")))?;
        json(
            self.api_with(reqwest::Method::PUT, "/me/presence", Some(&body))
                .await?,
            "/me/presence",
        )
        .await
    }

    /// Keep a status of the user's own.
    pub async fn create_status(&self, label: &str, state: &str) -> Result<OwnStatus, FcpError> {
        let body = serde_json::json!({ "label": label, "state": state });
        json(
            self.api_with(reqwest::Method::POST, "/me/presence/statuses", Some(&body))
                .await?,
            "/me/presence/statuses",
        )
        .await
    }

    /// Rename one of the user's statuses, or change its state.
    pub async fn update_status(
        &self,
        id: &str,
        label: Option<&str>,
        state: Option<&str>,
    ) -> Result<OwnStatus, FcpError> {
        let mut body = serde_json::Map::new();
        if let Some(label) = label {
            body.insert("label".into(), label.into());
        }
        if let Some(state) = state {
            body.insert("state".into(), state.into());
        }
        json(
            self.api_with(
                reqwest::Method::PUT,
                &format!("/me/presence/statuses/{id}"),
                Some(&body.into()),
            )
            .await?,
            "/me/presence/statuses",
        )
        .await
    }

    /// Delete one of the user's statuses; in use, the user is available.
    pub async fn delete_status(&self, id: &str) -> Result<(), FcpError> {
        self.api_with(
            reqwest::Method::DELETE,
            &format!("/me/presence/statuses/{id}"),
            None,
        )
        .await?;
        Ok(())
    }

    /// Put the user's statuses in this order: every id, once.
    pub async fn order_statuses(&self, ids: &[String]) -> Result<Vec<OwnStatus>, FcpError> {
        let body = serde_json::json!({ "ids": ids });
        let page: Statuses = json(
            self.api_with(
                reqwest::Method::PUT,
                "/me/presence/statuses/order",
                Some(&body),
            )
            .await?,
            "/me/presence/statuses/order",
        )
        .await?;
        Ok(page.data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_sends_only_what_it_changes() {
        let pick = PresenceUpdate {
            status: Some("away".into()),
            note: Some(Some("back at 2".into())),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&pick).unwrap(),
            serde_json::json!({"status": "away", "note": "back at 2"})
        );
        let clear = PresenceUpdate {
            note: Some(None),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&clear).unwrap(),
            serde_json::json!({"note": null})
        );
        let idle = PresenceUpdate {
            idle: Some(true),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&idle).unwrap(),
            serde_json::json!({"idle": true})
        );
    }

    #[test]
    fn fcps_answer_is_read() {
        let mine: MyPresence = serde_json::from_value(serde_json::json!({
            "presence": {"state": "on_call", "label": "WFH"},
            "status": "status-1", "status_state": "available",
            "note": null, "until": null,
            "statuses": [{"id": "status-1", "label": "WFH", "state": "available"}]
        }))
        .unwrap();
        assert_eq!(mine.presence.state, "on_call");
        assert_eq!(mine.presence.label.as_deref(), Some("WFH"));
        assert!(!mine.presence.idle);
        assert_eq!(mine.statuses[0].label, "WFH");
    }
}
