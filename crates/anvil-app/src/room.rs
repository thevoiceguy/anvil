//! FCP's video rooms (its `docs/SOFTPHONE_VIDEO.md` §2.6): joining one by
//! its number or a join link, who is in it and who speaks or shares, the
//! user's meetings and the links to send for them, and a host's controls.

use std::sync::Arc;

use anvil_core::{Event, FloorState, RoomAudio, RoomJoin, RoomMember};
use anvil_fcp::{JoinRole, RoomLayout};

use crate::command::RoomAction;
use crate::state::{CallView, Change, Direction, RoomMemberView, RoomView};
use crate::{update_call, CallState, Inner, Phone, PhoneError};

/// What `join` was given: a room's number, or a join link.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum JoinTarget {
    Number(String),
    Link {
        room: String,
        join: RoomJoin,
        host: bool,
    },
}

impl JoinTarget {
    /// A link (`fcp://join/…`, the browser's join page) is read for the
    /// room its token names; anything else is the room's number.
    pub(crate) fn parse(target: &str) -> Result<Self, PhoneError> {
        let target = target.trim();
        if target.is_empty() {
            return Err(PhoneError::Refused("join needs a room or a link".into()));
        }
        let link = target.starts_with("fcp://")
            || target.starts_with("https://")
            || target.starts_with("http://");
        if !link {
            return Ok(JoinTarget::Number(target.to_string()));
        }
        let join = RoomJoin::from_link(target)
            .ok_or_else(|| PhoneError::Refused("that is not a join link".into()))?;
        let details = join
            .details()
            .ok_or_else(|| PhoneError::Refused("that link names no room".into()))?;
        if details.expires_at != 0 && details.expires_at < crate::unix_now() {
            return Err(PhoneError::Refused("that link has expired".into()));
        }
        Ok(JoinTarget::Link {
            room: details.room,
            join,
            host: details.host,
        })
    }
}

impl Phone {
    /// Join a room by its number or a join link: a call with the room's
    /// screen offered from the start, video when asked and there is any.
    pub(crate) async fn join(&self, target: &str, video: bool) -> Result<u64, PhoneError> {
        let (number, join, token_host) = match JoinTarget::parse(target)? {
            JoinTarget::Number(n) => (n, RoomJoin::default(), false),
            JoinTarget::Link { room, join, host } => (room, join, host),
        };
        let owner = self.owns_room(&number);
        let dial = self.dial_target(&number);
        self.hold_the_others(None).await?;
        let media = self.media_for(video).await?;
        let call = self.anvil().await?.join_room(&dial, &join, media).await?;
        if let (true, Some(token)) = (token_host, join.token.clone()) {
            self.inner.room_tokens.lock().insert(call.0, token);
        }
        self.camera_later(call.0, video).await;
        let room = RoomView {
            number: number.clone(),
            host: owner || token_host,
            ..RoomView::default()
        };
        self.update_call(
            call.0,
            |c| {
                c.remote = dial.clone();
                c.display_name.get_or_insert_with(|| number.clone());
                c.video.camera = media.video && !media.receive_only;
                c.room = Some(room.clone());
            },
            || CallView {
                display_name: Some(number.clone()),
                room: Some(room.clone()),
                ..CallView::new(call.0, Direction::Outgoing, &dial, CallState::Dialing)
            },
        );
        Ok(call.0)
    }

    /// Whether the user owns the meeting at room `number`.
    fn owns_room(&self, number: &str) -> bool {
        self.inner
            .state
            .lock()
            .meetings
            .iter()
            .any(|m| m.room_number == number)
    }

    /// A link to send for one of the user's meetings (by id or room
    /// number): a guest's, or a host's.
    pub(crate) async fn join_link(&self, meeting: &str, host: bool) -> Result<String, PhoneError> {
        let fcp = Arc::clone(self.fcp("a join link")?);
        let id = self
            .inner
            .state
            .lock()
            .meetings
            .iter()
            .find(|m| m.id == meeting || m.room_number == meeting)
            .map(|m| m.id.clone())
            .unwrap_or_else(|| meeting.to_string());
        let role = if host {
            JoinRole::Host
        } else {
            JoinRole::Guest
        };
        Ok(fcp.join_link(&id, role, None, None).await?.link())
    }

    /// Share our screen in the room `call` is in, or stop. The floor is
    /// taken by sending: the room's notice says whether it was granted.
    pub(crate) async fn share(&self, call: Option<u64>, on: bool) -> Result<(), PhoneError> {
        let id = self.pick(call, "no call in a room", |c| {
            c.room.is_some() && c.state == CallState::Connected && !c.held
        })?;
        let mark = |sharing: bool| {
            self.update_call(
                id,
                |c| {
                    if let Some(r) = c.room.as_mut() {
                        r.sharing = sharing;
                        if sharing {
                            r.floor_refused = None;
                        }
                    }
                },
                || crate::outgoing(id),
            )
        };
        // Marked before the re-INVITE: the room's notice can come before
        // its answer does. Our stop's notice is then not read as the room
        // ending the share, and a refusal of our share is acted on.
        let was = self
            .inner
            .state
            .lock()
            .call(id)
            .and_then(|c| c.room.as_ref().map(|r| r.sharing))
            .unwrap_or(false);
        mark(on);
        let done = self
            .anvil()
            .await?
            .share_screen(anvil_core::CallId(id), on)
            .await;
        if done.is_err() {
            mark(was);
        }
        Ok(done?)
    }

    /// A host's control over the room `call` is in.
    pub(crate) async fn room_control(
        &self,
        call: Option<u64>,
        action: RoomAction,
    ) -> Result<(), PhoneError> {
        let fcp = Arc::clone(self.fcp("a host's controls")?);
        let id = self.pick(call, "no call in a room", |c| {
            c.room.is_some() && c.state == CallState::Connected
        })?;
        let room = self
            .inner
            .state
            .lock()
            .call(id)
            .and_then(|c| c.room.clone())
            .ok_or_else(|| PhoneError::Refused(format!("call {id} is not in a room")))?;
        if !room.host {
            return Err(PhoneError::Refused(
                "only the room's host has its controls".into(),
            ));
        }
        let token = self.inner.room_tokens.lock().get(&id).cloned();
        let host = fcp.room_host(&room.number, token.as_deref());
        match action {
            RoomAction::Layout { grid } => {
                host.set_layout(if grid {
                    RoomLayout::Grid
                } else {
                    RoomLayout::Speaker
                })
                .await?
            }
            RoomAction::Pin { member: Some(m) } => host.pin(&m).await?,
            RoomAction::Pin { member: None } => host.unpin().await?,
            RoomAction::Spotlight { member: Some(m) } => host.spotlight(&m).await?,
            RoomAction::Spotlight { member: None } => host.unspotlight().await?,
            RoomAction::Mute { member, on } => host.mute(&member, on).await?,
            RoomAction::MemberVideo { member, on } => host.participant_video(&member, on).await?,
            RoomAction::Remove { member } => host.remove(&member).await?,
            RoomAction::StopShare => host.stop_content().await?,
        }
        Ok(())
    }
}

impl Inner {
    /// The user's meetings as FCP gave them: the calls in one of their
    /// rooms are the host's.
    pub(crate) fn set_meetings(&self, meetings: Vec<anvil_fcp::Meeting>) {
        let (count, hosted) = {
            let mut state = self.state.lock();
            if state.meetings == meetings {
                return;
            }
            let hosted: Vec<u64> = state
                .calls
                .iter()
                .filter(|c| {
                    c.room.as_ref().is_some_and(|r| {
                        !r.host && meetings.iter().any(|m| m.room_number == r.number)
                    })
                })
                .map(|c| c.id)
                .collect();
            state.meetings = meetings;
            (state.meetings.len() as u32, hosted)
        };
        for id in hosted {
            update_call(
                self,
                id,
                |c| {
                    if let Some(r) = c.room.as_mut() {
                        r.host = true;
                    }
                },
                || crate::outgoing(id),
            );
        }
        let _ = self.changes.send(Change::Meetings { count });
    }
}

/// The user's meetings, from an FCP that has them.
pub(crate) async fn refresh_meetings(inner: &Inner, fcp: &anvil_fcp::FcpClient) {
    match fcp.meetings().await {
        Ok(meetings) => inner.set_meetings(meetings),
        Err(e) => tracing::debug!(%e, "meetings not read"),
    }
}

fn member_view(m: &RoomMember) -> RoomMemberView {
    RoomMemberView {
        id: m.id.clone(),
        name: m.name.clone(),
        host: m.host,
        audio: match m.audio {
            RoomAudio::Sending => "sending",
            RoomAudio::Muted => "muted",
            RoomAudio::Held => "held",
        }
        .to_string(),
        camera: m.camera.clone(),
        speaking: m.speaking,
        presenting: m.presenting,
    }
}

/// Tell the room our share is over (a re-INVITE receiving only), off the
/// event loop.
fn stop_share(inner: &Arc<Inner>, call: u64) {
    let inner = Arc::clone(inner);
    tokio::spawn(async move {
        let anvil = inner.anvil.read().await;
        if let Some(anvil) = anvil.as_ref() {
            if let Err(e) = anvil.share_screen(anvil_core::CallId(call), false).await {
                tracing::info!(call, %e, "the room was not told the share ended");
            }
        }
    });
}

/// A room's event, on the call it is for; false for any other event.
pub(crate) fn on_event(inner: &Arc<Inner>, event: &Event) -> bool {
    let in_room = |id: u64| {
        inner
            .state
            .lock()
            .call(id)
            .is_some_and(|c| c.room.is_some())
    };
    match event {
        Event::RoomRoster {
            call,
            members,
            speaker,
        } if in_room(call.0) => {
            let members: Vec<RoomMemberView> = members.iter().map(member_view).collect();
            let presenter = members
                .iter()
                .find(|m| m.presenting)
                .map(|m| m.name.clone());
            update_call(
                inner,
                call.0,
                |c| {
                    if let Some(r) = c.room.as_mut() {
                        r.members = members;
                        r.speaker = speaker.clone();
                        r.presenter = presenter;
                    }
                },
                || crate::outgoing(call.0),
            );
        }
        Event::RoomFloor { call, notice } if in_room(call.0) => update_call(
            inner,
            call.0,
            |c| {
                let Some(r) = c.room.as_mut() else { return };
                match notice.state {
                    FloorState::Granted => {
                        r.presenter = notice.presenter.clone();
                        if notice.mine {
                            r.floor_refused = None;
                        }
                    }
                    FloorState::Released => {
                        r.presenter = None;
                        if notice.mine {
                            // Our own stop is not a reason to show.
                            r.floor_refused = notice.reason.clone().filter(|_| r.sharing);
                        }
                    }
                    FloorState::Refused => {
                        if notice.mine {
                            r.floor_refused =
                                Some(notice.reason.clone().unwrap_or_else(|| "refused".into()));
                        }
                    }
                }
                // Our share turned down, or taken back by a host or the
                // room: Anvil stopped the screen; the room is told, so a
                // share after this is a new one.
                if notice.mine
                    && r.sharing
                    && matches!(notice.state, FloorState::Refused | FloorState::Released)
                {
                    r.sharing = false;
                    stop_share(inner, call.0);
                }
            },
            || crate::outgoing(call.0),
        ),
        Event::RoomRoster { .. } | Event::RoomFloor { .. } => {}
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(room: &str, role: &str, exp: u64) -> String {
        use base64::Engine as _;
        let payload = format!(
            r#"{{"v":1,"tenant":"t","room":"{room}","name":"Jo","role":"{role}","exp":{exp},"nonce":"n"}}"#
        );
        format!(
            "{}.sig",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload)
        )
    }

    #[test]
    fn a_number_is_dialed_and_a_link_read_for_its_room() {
        assert_eq!(
            JoinTarget::parse(" 3000 ").unwrap(),
            JoinTarget::Number("3000".into())
        );
        let later = crate::unix_now() + 600;
        let link = format!("fcp://join/{}", token("3000", "host", later));
        match JoinTarget::parse(&link).unwrap() {
            JoinTarget::Link { room, join, host } => {
                assert_eq!(room, "3000");
                assert!(host);
                assert!(join.token.is_some());
            }
            other => panic!("{other:?}"),
        }
        let page = format!(
            "https://meet.example.com/join?token={}",
            token("3001", "guest", later)
        );
        assert!(matches!(
            JoinTarget::parse(&page).unwrap(),
            JoinTarget::Link { ref room, host: false, .. } if room == "3001"
        ));
        let old = format!("fcp://join/{}", token("3000", "guest", 1));
        assert!(JoinTarget::parse(&old).is_err());
        assert!(JoinTarget::parse("fcp://join/nonsense").is_err());
        assert!(JoinTarget::parse("").is_err());
    }
}
