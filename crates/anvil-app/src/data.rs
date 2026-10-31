//! The user's data from FCP (`docs/APP.md` §3): recent calls, the directory
//! with presence, voicemail, calling settings and the user's own contacts —
//! read when the phone starts, and again when FCP's live events
//! (`/me/events`) say they changed. FCP says nothing when a contact changes
//! elsewhere, so contacts are read again when the events reconnect and when
//! the app asks (`refresh`).

use std::sync::{Arc, Weak};
use std::time::Duration;

use anvil_fcp::{
    CallQuery, CallRecord, ContactFields, DirectoryEntry, FcpClient, PersonalContact, UserEvent,
    VoicemailMessage,
};

use crate::state::{Change, Contact, ContactNumber, Direction, Person, Recent, Voicemail};
use crate::Inner;

/// How many recent calls the phone keeps.
pub(crate) const RECENTS: u32 = 50;
/// How many voicemail messages the phone keeps.
const MESSAGES: u32 = 100;
/// The directory is read a page at a time, up to this many people.
const PAGE: u32 = 200;
const PEOPLE: usize = 5000;
/// The most contacts FCP keeps for one user.
const CONTACTS: usize = 5000;
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
        mos_hundredths: r.quality.map(|q| {
            let worst = q.far_mos.map_or(q.mos, |far| far.min(q.mos));
            (worst * 100.0).round() as u16
        }),
        poor: r.quality.is_some_and(|q| q.poor),
    }
}

/// What to tell FCP of a call that has ended: its last second's figures,
/// by the server's id for it. Nothing for a call never measured or never
/// named by the server.
pub(crate) fn quality_report(
    call: &crate::state::CallView,
    ended_at: u64,
) -> Option<(String, anvil_fcp::CallQualityReport)> {
    let server_call = call.server_call.clone()?;
    let q = call.quality.as_deref()?;
    if q.mos_hundredths == 0 {
        return None;
    }
    let pct = |permille: u32| permille as f32 / 10.0;
    let mos = |hundredths: u16| (f32::from(hundredths) / 100.0).clamp(1.0, 4.5);
    Some((
        server_call,
        anvil_fcp::CallQualityReport {
            direction: match call.direction {
                crate::state::Direction::Incoming => "incoming",
                crate::state::Direction::Outgoing => "outgoing",
            }
            .to_string(),
            codec: call.codec.clone().unwrap_or_else(|| "unknown".into()),
            srtp: call.encrypted,
            duration_seconds: call
                .connected_at
                .map(|at| ended_at.saturating_sub(at).min(u64::from(u32::MAX)) as u32),
            packets_sent: q.packets_sent,
            packets_received: q.rtp_received,
            packets_expected: q.packets_expected,
            packets_lost: q.packets_lost.min(q.packets_expected),
            jitter_ms: q.jitter_ms as f32,
            rtt_ms: q.rtt_ms.map(|r| r as f32),
            far_loss_pct: q.far_loss_permille.map(|p| pct(p).min(100.0)),
            far_jitter_ms: q.far_jitter_ms.map(|j| j as f32),
            far_lost: q.far_lost,
            mos: mos(q.mos_hundredths),
            far_mos: q.far_mos_hundredths.map(mos),
            local_rtp: q.local_rtp.map(|a| a.to_string()),
            remote_rtp: q.remote_rtp.map(|a| a.to_string()),
        },
    ))
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
        presence_label: e.presence_label.clone(),
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

/// One of the account's contacts, as the phone shows it.
pub(crate) fn contact(c: &PersonalContact) -> Contact {
    Contact {
        id: c.id.clone(),
        on_account: true,
        name: c.name.clone(),
        company: c.company.clone(),
        numbers: c
            .numbers
            .iter()
            .map(|n| ContactNumber {
                label: n.label.clone(),
                number: n.number.clone(),
            })
            .collect(),
        email: c.email.clone(),
        notes: c.notes.clone(),
        ringtone: c.ringtone.clone(),
    }
}

/// A contact as FCP takes it.
pub(crate) fn contact_fields(c: &Contact) -> ContactFields {
    ContactFields {
        name: c.name.clone(),
        company: c.company.clone(),
        numbers: c
            .numbers
            .iter()
            .map(|n| anvil_fcp::ContactNumber {
                label: n.label.clone(),
                number: n.number.clone(),
            })
            .collect(),
        email: c.email.clone(),
        notes: c.notes.clone(),
        ringtone: c.ringtone.clone(),
    }
}

/// Read everything again.
pub(crate) async fn refresh_all(inner: &Inner, fcp: &FcpClient) {
    tokio::join!(
        refresh_softphone(inner, fcp),
        refresh_calling(inner, fcp),
        refresh_recents(inner, fcp),
        refresh_people(inner, fcp),
        refresh_voicemail(inner, fcp),
        refresh_contacts(inner, fcp),
        refresh_presence(inner, fcp),
    );
}

/// The user's presence, from an FCP that keeps it.
pub(crate) async fn refresh_presence(inner: &Inner, fcp: &FcpClient) {
    match fcp.presence().await {
        Ok(p) => inner.set_presence(p.map(Into::into)),
        Err(e) => tracing::debug!(%e, "presence not read"),
    }
}

/// What FCP tells the app: the feature codes, whether the user may record,
/// their listing in the directory.
pub(crate) async fn refresh_softphone(inner: &Inner, fcp: &FcpClient) {
    match fcp.softphone().await {
        Ok(config) => {
            inner.set_may_record(config.may_record);
            inner.set_directory(Some(crate::DirectoryListing {
                listed: config.directory_listed,
                may_change: config.may_change_directory_listing,
            }));
            *inner.features.lock() = config.feature_codes;
            inner.set_lines(config.lines, config.may_rename_lines).await;
        }
        Err(e) => tracing::debug!(%e, "softphone settings not read"),
    }
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
            inner.set_recents(page.data.iter().map(recent).collect());
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

/// The account's contacts, a page at a time. An FCP before v0.25.467
/// keeps none (404): the account has none to show.
pub(crate) async fn refresh_contacts(inner: &Inner, fcp: &FcpClient) {
    let mut contacts = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        match fcp.contacts(None, Some(PAGE), cursor.as_deref()).await {
            Ok(page) => {
                contacts.extend(page.data.iter().map(contact));
                cursor = page.next_cursor;
                if cursor.is_none() || contacts.len() >= CONTACTS {
                    break;
                }
            }
            Err(e) => {
                tracing::debug!(%e, "contacts not read");
                return;
            }
        }
    }
    inner.set_account_contacts(contacts);
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
                Refresh::Contacts => refresh_contacts(&inner, &fcp).await,
                Refresh::Presence { username } => {
                    if fcp.session().await.username.eq_ignore_ascii_case(&username) {
                        refresh_presence(&inner, &fcp).await;
                    }
                }
            }
        });
    };
    let text = |field: &str| event.body.get(field).and_then(|v| v.as_str());
    match event.name.as_str() {
        "call.ended" | "call.failed" => spawn(Refresh::Recents),
        name if name.starts_with("voicemail.") => spawn(Refresh::Voicemail),
        "user.calling_changed" => spawn(Refresh::Calling),
        "user.contact_changed" => spawn(Refresh::Contacts),
        // A person's presence (FCP v0.25.473 on): theirs in the
        // directory, and the user's own read again.
        "presence.user_changed" => {
            if let (Some(username), Some(state)) = (text("username"), text("state")) {
                let label = text("label").map(str::to_string);
                inner.update_person_by_key(username, |p| {
                    p.presence = Some(state.to_string());
                    p.presence_label = label;
                });
                spawn(Refresh::Presence {
                    username: username.to_string(),
                });
            }
        }
        // An address's presence: an FCP that keeps a person's says it
        // better, in `presence.user_changed`.
        "presence.status_changed" if inner.state.lock().presence.is_none() => {
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
    Contacts,
    /// The user's own presence, when an event names them.
    Presence {
        username: String,
    },
}

/// The user part of an address (`sip:alice@example.com` → `alice`,
/// `tel:+61412345678` → `+61412345678`).
pub(crate) fn user_of(aor: &str) -> &str {
    let rest = aor
        .strip_prefix("sips:")
        .or_else(|| aor.strip_prefix("sip:"))
        .or_else(|| aor.strip_prefix("tel:"))
        .unwrap_or(aor);
    rest.split(['@', ';']).next().unwrap_or(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_measured_call_is_reported_by_the_servers_id_and_an_unmeasured_one_is_not() {
        use crate::state::{CallState, CallView, Direction, Quality};
        let mut call = CallView::new(7, Direction::Outgoing, "sip:1002@x", CallState::Connected);
        call.codec = Some("opus".into());
        call.encrypted = true;
        call.connected_at = Some(1_000);
        assert!(quality_report(&call, 1_060).is_none(), "nothing measured");
        call.quality = Some(Box::new(Quality {
            jitter_ms: 3,
            packet_loss_permille: 2,
            rtt_ms: Some(42),
            packets_received: 3000,
            packets_played: 3000,
            packets_dropped: 0,
            unexpected_pt: None,
            last_source: None,
            recent_loss_permille: 0,
            packets_sent: 3001,
            bytes_sent: 1,
            rtp_received: 2994,
            bytes_received: 1,
            packets_expected: 3000,
            packets_lost: 6,
            send_kbps: 0,
            recv_kbps: 0,
            send_pps: 0,
            recv_pps: 0,
            far_loss_permille: Some(15),
            far_jitter_ms: Some(2),
            far_lost: Some(9),
            rtcp: true,
            rtcp_sent: 12,
            rtcp_received: 12,
            clock_rate: 48_000,
            ptime_ms: 20,
            local_rtp: Some("192.168.1.20:40000".parse().unwrap()),
            remote_rtp: Some("203.0.113.5:30000".parse().unwrap()),
            mos_hundredths: 431,
            far_mos_hundredths: Some(402),
        }));
        assert!(
            quality_report(&call, 1_060).is_none(),
            "the server never named it"
        );
        call.server_call = Some("call-abc".into());
        let (id, r) = quality_report(&call, 1_060).unwrap();
        assert_eq!(id, "call-abc");
        assert_eq!(r.direction, "outgoing");
        assert_eq!(r.codec, "opus");
        assert!(r.srtp);
        assert_eq!(r.duration_seconds, Some(60));
        assert_eq!(
            (r.packets_received, r.packets_expected, r.packets_lost),
            (2994, 3000, 6)
        );
        assert_eq!(r.far_loss_pct, Some(1.5));
        assert_eq!((r.mos, r.far_mos), (4.31, Some(4.02)));
        assert_eq!(r.remote_rtp.as_deref(), Some("203.0.113.5:30000"));
    }

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
