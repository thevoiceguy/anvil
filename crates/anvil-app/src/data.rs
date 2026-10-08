//! The user's data from FCP (`docs/APP.md` §3): recent calls, the directory
//! with presence, voicemail and calling settings — read when the phone
//! starts, and again when FCP's live events (`/me/events`) say they changed.

use std::sync::{Arc, Weak};
use std::time::Duration;

use anvil_fcp::{CallQuery, CallRecord, DirectoryEntry, FcpClient, UserEvent, VoicemailMessage};

use crate::state::{Change, Direction, Person, Recent, Voicemail};
use crate::Inner;

/// How many recent calls the phone keeps.
pub(crate) const RECENTS: u32 = 50;
/// How many voicemail messages the phone keeps.
const MESSAGES: u32 = 100;
/// The directory is read a page at a time, up to this many people.
const PAGE: u32 = 200;
const PEOPLE: usize = 5000;
/// A call's record is written a moment after the call ends: the recents are
/// read again after this long.
pub(crate) const RECORD_LAG: Duration = Duration::from_millis(1500);

pub(crate) fn recent(r: &CallRecord) -> Recent {
    Recent {
        id: r.cdr_id.clone(),
        direction: if r.direction == "incoming" {
            Direction::Incoming
        } else {
            Direction::Outgoing
        },
        remote: r.other_party.clone(),
        display_name: r.other_party_name.clone(),
        missed: r.missed,
        started_at: r.start_time.clone(),
        duration_secs: r.duration_seconds,
    }
}

pub(crate) fn person(e: &DirectoryEntry, favourite: impl Fn(&str) -> bool) -> Option<Person> {
    let key = e.username.clone().or_else(|| e.extension.clone())?;
    Some(Person {
        name: e
            .display_name
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| key.clone()),
        favourite: favourite(&key),
        key,
        extension: e.extension.clone(),
        department: e.department.clone(),
        job_title: e.job_title.clone(),
        presence: e.presence.clone(),
        on_call: false,
    })
}

pub(crate) fn voicemail(m: &VoicemailMessage) -> Voicemail {
    Voicemail {
        id: m.id.clone(),
        caller: m.caller.clone(),
        caller_name: m.caller_name.clone(),
        new: m.status == "new",
        urgent: m.priority == "urgent",
        duration_secs: m.duration,
        transcription: m.transcription.clone(),
        received_at: m.created_at.clone(),
    }
}

/// Read everything again.
pub(crate) async fn refresh_all(inner: &Inner, fcp: &FcpClient) {
    tokio::join!(
        refresh_calling(inner, fcp),
        refresh_recents(inner, fcp),
        refresh_people(inner, fcp),
        refresh_voicemail(inner, fcp),
    );
}

pub(crate) async fn refresh_calling(inner: &Inner, fcp: &FcpClient) {
    match fcp.calling().await {
        Ok(settings) => inner.set_calling(settings),
        Err(e) => tracing::debug!(%e, "calling settings not read"),
    }
}

pub(crate) async fn refresh_recents(inner: &Inner, fcp: &FcpClient) {
    let query = CallQuery {
        limit: Some(RECENTS),
        ..Default::default()
    };
    match fcp.calls(&query).await {
        Ok(page) => {
            let recents: Vec<Recent> = page.data.iter().map(recent).collect();
            let missed = recents.iter().filter(|r| r.missed).count() as u32;
            inner.state.lock().recents = recents;
            let _ = inner.changes.send(Change::Recents { missed });
        }
        Err(e) => tracing::debug!(%e, "recent calls not read"),
    }
}

pub(crate) async fn refresh_people(inner: &Inner, fcp: &FcpClient) {
    let mut entries = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        match fcp.directory(None, Some(PAGE), cursor.as_deref()).await {
            Ok(page) => {
                entries.extend(page.data);
                cursor = page.next_cursor;
                if cursor.is_none() || entries.len() >= PEOPLE {
                    break;
                }
            }
            Err(e) => {
                tracing::debug!(%e, "directory not read");
                return;
            }
        }
    }
    let favourites = inner.local.lock().favourites.clone();
    let mut people: Vec<Person> = entries
        .iter()
        .filter_map(|e| person(e, |key| favourites.contains(key)))
        .collect();
    people.sort_by_key(|p| p.name.to_lowercase());
    let count = people.len() as u32;
    {
        // Busy lamps live only in events: keep the ones already lit.
        let mut state = inner.state.lock();
        for p in &mut people {
            p.on_call = state
                .people
                .iter()
                .any(|old| old.key == p.key && old.on_call);
        }
        state.people = people;
    }
    let _ = inner.changes.send(Change::People { count });
}

pub(crate) async fn refresh_voicemail(inner: &Inner, fcp: &FcpClient) {
    match fcp.voicemail(Some(MESSAGES), None).await {
        Ok(page) => {
            let messages: Vec<Voicemail> = page.data.iter().map(voicemail).collect();
            let new = messages.iter().filter(|m| m.new).count() as u32;
            let total = messages.len() as u32;
            inner.state.lock().voicemail = messages;
            let _ = inner.changes.send(Change::Voicemail { new, total });
        }
        // A user without a mailbox has none to read.
        Err(e) => tracing::debug!(%e, "voicemail not read"),
    }
}

/// Follow `/me/events` while the phone runs, connecting again when the
/// stream ends: after a gap everything is read again, since what changed
/// meanwhile was missed.
pub(crate) async fn follow(inner: Weak<Inner>, fcp: Arc<FcpClient>) {
    let mut backoff = Duration::from_secs(1);
    let mut first = true;
    loop {
        match fcp.events().await {
            Ok(mut events) => {
                backoff = Duration::from_secs(1);
                if !first {
                    let Some(inner) = inner.upgrade() else { return };
                    refresh_all(&inner, &fcp).await;
                }
                first = false;
                while let Some(event) = events.next().await {
                    let Some(inner) = inner.upgrade() else { return };
                    on_event(&inner, &fcp, &event);
                }
                tracing::debug!("the user's events ended; connecting again");
            }
            Err(e) => tracing::debug!(%e, "the user's events not reached"),
        }
        if inner.strong_count() == 0 {
            return;
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

/// What one of the user's events changes.
fn on_event(inner: &Arc<Inner>, fcp: &Arc<FcpClient>, event: &UserEvent) {
    let spawn = |what: Refresh| {
        let (inner, fcp) = (Arc::clone(inner), Arc::clone(fcp));
        tokio::spawn(async move {
            match what {
                Refresh::Recents => {
                    tokio::time::sleep(RECORD_LAG).await;
                    refresh_recents(&inner, &fcp).await;
                }
                Refresh::Voicemail => refresh_voicemail(&inner, &fcp).await,
                Refresh::Calling => refresh_calling(&inner, &fcp).await,
            }
        });
    };
    let text = |field: &str| event.body.get(field).and_then(|v| v.as_str());
    match event.name.as_str() {
        "call.ended" | "call.failed" => spawn(Refresh::Recents),
        name if name.starts_with("voicemail.") => spawn(Refresh::Voicemail),
        "user.calling_settings_changed" => spawn(Refresh::Calling),
        "presence.status_changed" => {
            if let (Some(aor), Some(state)) = (text("aor"), text("new_state")) {
                inner.update_person(aor, |p| p.presence = Some(state.to_string()));
            }
        }
        "presence.dialog_changed" => {
            if let (Some(aor), Some(state)) = (text("aor"), text("dialog_state")) {
                let on_call = !matches!(state, "terminated" | "idle");
                inner.update_person(aor, |p| p.on_call = on_call);
            }
        }
        _ => {}
    }
}

enum Refresh {
    Recents,
    Voicemail,
    Calling,
}

/// The user part of an address (`sip:alice@example.com` → `alice`).
pub(crate) fn user_of(aor: &str) -> &str {
    let rest = aor
        .strip_prefix("sips:")
        .or_else(|| aor.strip_prefix("sip:"))
        .unwrap_or(aor);
    rest.split(['@', ';']).next().unwrap_or(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_names_its_user() {
        assert_eq!(user_of("sip:alice@example.com"), "alice");
        assert_eq!(user_of("sips:1002@pbx;transport=tls"), "1002");
        assert_eq!(user_of("bob"), "bob");
    }

    #[test]
    fn a_directory_entry_is_a_person() {
        let entry: DirectoryEntry = serde_json::from_value(serde_json::json!({
            "kind": "user", "user_id": "u1", "username": "alice",
            "display_name": "Alice Smith", "extension": "1001", "email": null,
            "department": "Sales", "job_title": null, "presence": "available",
            "presence_note": null,
        }))
        .unwrap();
        let p = person(&entry, |k| k == "alice").unwrap();
        assert_eq!(p.key, "alice");
        assert_eq!(p.name, "Alice Smith");
        assert_eq!(p.dial(), "1001");
        assert!(p.favourite);
        assert_eq!(p.presence.as_deref(), Some("available"));

        let queue: DirectoryEntry = serde_json::from_value(serde_json::json!({
            "kind": "extension", "user_id": null, "username": null,
            "display_name": "", "extension": "5000", "email": null,
            "department": null, "job_title": null, "presence": null,
            "presence_note": null,
        }))
        .unwrap();
        let q = person(&queue, |_| false).unwrap();
        assert_eq!((q.key.as_str(), q.name.as_str()), ("5000", "5000"));
    }
}
