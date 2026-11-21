//! FCP's video rooms (its `docs/SOFTPHONE_VIDEO.md` §2.6): joining one by
//! its number or a join link, who is in it and who speaks or shares, the
//! user's meetings and the links to send for them, and a host's controls.

use std::sync::Arc;

use anvil_core::{Event, FloorState, RoomAudio, RoomJoin, RoomMember};
use anvil_fcp::{JoinRole, RoomLayout};

use crate::command::{AudienceAction, RoomAction};
use crate::state::{
    CallView, Change, ChatItem, Direction, QuestionItem, RoomMemberView, RoomStreamView, RoomView,
};
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

    /// What the room `call` is in sends us, where it forwards each
    /// person's stream: `members` and with `speaker` whoever speaks, each
    /// at the height its tile has; `pinned` large; neither, the room's
    /// default.
    pub(crate) async fn set_view(
        &self,
        call: Option<u64>,
        members: Vec<String>,
        speaker: bool,
        pinned: Option<String>,
    ) -> Result<(), PhoneError> {
        let id = self.pick(call, "no call in a room", |c| {
            c.room.is_some() && c.state == CallState::Connected
        })?;
        let view = view_of(&members, speaker, pinned.as_deref());
        Ok(self
            .anvil()
            .await?
            .set_room_view(anvil_core::CallId(id), &view)
            .await?)
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
            RoomAction::GoLive => host.go_live().await?,
            RoomAction::EndWebinar => host.end_webinar().await?,
            RoomAction::LowerHand { member } => host.lower_hand(&member).await?,
            RoomAction::LowerAllHands => host.lower_all_hands().await?,
            RoomAction::Stage { member, on } => host.stage(&member, on).await?,
            RoomAction::MuteChat { member, on } => host.mute_chat(&member, on).await?,
        }
        Ok(())
    }

    /// Our voice in the room `call` is in: an INFO in the call. The room
    /// answers in what it tells us next (our hand, the questions, chat).
    pub(crate) async fn audience(
        &self,
        call: Option<u64>,
        action: AudienceAction,
    ) -> Result<(), PhoneError> {
        use anvil_core::{ChatRequest, ChatTo, QaRequest};
        let id = self.pick(call, "no call in a room", |c| {
            c.room.is_some() && c.state == CallState::Connected
        })?;
        let anvil = self.anvil().await?;
        let call = anvil_core::CallId(id);
        match action {
            AudienceAction::Hand { raised } => {
                anvil.room_qa(call, &QaRequest::Hand { raised }).await?
            }
            AudienceAction::Ask { text, anonymous } => {
                anvil
                    .room_qa(call, &QaRequest::Ask { text, anonymous })
                    .await?
            }
            AudienceAction::Upvote { id, on } => {
                anvil.room_qa(call, &QaRequest::Upvote { id, on }).await?
            }
            AudienceAction::Publish { id } => {
                anvil.room_qa(call, &QaRequest::Publish { id }).await?
            }
            AudienceAction::Answer { id, text, live } => {
                anvil
                    .room_qa(call, &QaRequest::Answer { id, text, live })
                    .await?
            }
            AudienceAction::Dismiss { id } => {
                anvil.room_qa(call, &QaRequest::Dismiss { id }).await?
            }
            AudienceAction::Chat { text, to_hosts } => {
                let to = if to_hosts {
                    ChatTo::Hosts
                } else {
                    ChatTo::Room
                };
                anvil
                    .room_chat(call, &ChatRequest::Post { text, to })
                    .await?
            }
            AudienceAction::DeleteMessage { id } => {
                anvil.room_chat(call, &ChatRequest::Delete { id }).await?
            }
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

/// The view for a layout of `members` (and the speaker): one large (the
/// pinned, else the speaker) and kept first, the rest small beside it;
/// without one large, each as tall as a grid of that many lets a tile be.
fn view_of(members: &[String], speaker: bool, pinned: Option<&str>) -> anvil_core::RoomView {
    if members.is_empty() && !speaker && pinned.is_none() {
        return anvil_core::RoomView::default();
    }
    let large = |target| anvil_core::ViewStream {
        target,
        max_height: Some(720),
        priority: Some(1),
    };
    let mut streams = Vec::new();
    if let Some(p) = pinned {
        streams.push(large(anvil_core::ViewTarget::Member(p.to_string())));
    } else if speaker {
        streams.push(large(anvil_core::ViewTarget::Speaker));
    }
    let others: Vec<&String> = members
        .iter()
        .filter(|m| Some(m.as_str()) != pinned)
        .collect();
    let tile = if !streams.is_empty() {
        180
    } else {
        match others.len() {
            1 => 720,
            2..=4 => 360,
            _ => 180,
        }
    };
    streams.extend(others.into_iter().map(|m| anvil_core::ViewStream {
        target: anvil_core::ViewTarget::Member(m.clone()),
        max_height: Some(tile),
        priority: None,
    }));
    anvil_core::RoomView {
        max_streams: Some(streams.len().min(25) as u8),
        streams: Some(streams),
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
        paused: m.paused,
        spotlight: m.spotlight,
        panelist: m.panelist,
        attendee: m.attendee,
        hand: m.hand.clone(),
    }
}

/// The most chat a call keeps.
const CHAT_KEPT: usize = 500;

fn question_item(q: &anvil_core::Question) -> QuestionItem {
    QuestionItem {
        id: q.id.clone(),
        text: q.text.clone(),
        asker: q.asker.clone(),
        mine: q.mine,
        status: match q.status {
            anvil_core::QuestionStatus::Pending => "pending",
            anvil_core::QuestionStatus::Answered => "answered",
            anvil_core::QuestionStatus::Dismissed => "dismissed",
        }
        .to_string(),
        published: q.published,
        upvotes: q.upvotes,
        upvoted: q.upvoted,
        answer: q.answer.clone(),
        answered_live: q.answered_live,
        asked_at: q.asked_at.clone(),
    }
}

/// The questions the room told merged into what we had: changed ones
/// replaced, new ones added, dismissed ones dropped.
fn merge_questions(kept: &mut Vec<QuestionItem>, told: &[anvil_core::Question]) {
    for q in told {
        let at = kept.iter().position(|k| k.id == q.id);
        match (at, q.status == anvil_core::QuestionStatus::Dismissed) {
            (Some(i), true) => {
                kept.remove(i);
            }
            (Some(i), false) => kept[i] = question_item(q),
            (None, false) => kept.push(question_item(q)),
            (None, true) => {}
        }
    }
}

/// Chat the room told merged in: new messages added, deleted ones dropped,
/// the oldest let go past [`CHAT_KEPT`].
fn merge_chat(kept: &mut Vec<ChatItem>, told: &[anvil_core::ChatMessage]) {
    for m in told {
        let at = kept.iter().position(|k| k.id == m.id);
        if m.deleted {
            if let Some(i) = at {
                kept.remove(i);
            }
            continue;
        }
        let item = ChatItem {
            id: m.id.clone(),
            text: m.text.clone(),
            sender: m.sender.clone(),
            sender_id: m.sender_id.clone(),
            from_stage: m.sender_moderates,
            mine: m.mine,
            to_hosts: m.to == anvil_core::ChatTo::Hosts,
            sent_at: m.sent_at.clone(),
        };
        match at {
            Some(i) => kept[i] = item,
            None => kept.push(item),
        }
    }
    if kept.len() > CHAT_KEPT {
        let over = kept.len() - CHAT_KEPT;
        kept.drain(..over);
    }
}

/// An attendee is not heard until brought to the stage: the microphone is
/// muted in the app while they listen, and theirs to unmute once on it.
fn hold_the_microphone(inner: &Arc<Inner>, call: u64, muted: bool) {
    let inner2 = Arc::clone(inner);
    tokio::spawn(async move {
        let anvil = inner2.anvil.read().await;
        if let Some(anvil) = anvil.as_ref() {
            match anvil.mute(anvil_core::CallId(call), muted) {
                Ok(()) => update_call(&inner2, call, |c| c.muted = muted, || crate::outgoing(call)),
                Err(e) => tracing::info!(call, %e, "the microphone was not muted for the audience"),
            }
        }
    });
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
            forwards,
            audience,
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
                        r.forwards = *forwards;
                        r.audience = *audience;
                    }
                },
                || crate::outgoing(call.0),
            );
        }
        Event::RoomStreams { call, streams } if in_room(call.0) => {
            let streams: Vec<RoomStreamView> = streams
                .iter()
                .map(|s| RoomStreamView {
                    stream: s.stream.0,
                    member: s.member.clone(),
                    speaker: s.speaker,
                })
                .collect();
            update_call(
                inner,
                call.0,
                |c| {
                    if let Some(r) = c.room.as_mut() {
                        r.streams = streams;
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
        Event::RoomWebinar { call, state } if in_room(call.0) => {
            let role = state.role.map(|r| {
                match r {
                    anvil_core::RoomRole::Host => "host",
                    anvil_core::RoomRole::Panelist => "panelist",
                    anvil_core::RoomRole::Attendee => "attendee",
                    anvil_core::RoomRole::Guest => "guest",
                }
                .to_string()
            });
            let phase = match state.phase {
                anvil_core::WebinarPhase::Practice => "practice",
                anvil_core::WebinarPhase::Live => "live",
                anvil_core::WebinarPhase::Ended => "ended",
            };
            // Newly an attendee off the stage: not heard, so muted.
            let mut mute = false;
            update_call(
                inner,
                call.0,
                |c| {
                    if let Some(r) = c.room.as_mut() {
                        let was = r.role.clone();
                        r.webinar = Some(phase.to_string());
                        r.lobby = state.lobby;
                        if role.is_some() {
                            r.role = role.clone();
                        }
                        if role.as_deref() == Some("host") {
                            r.host = true;
                        }
                        mute = r.role.as_deref() == Some("attendee")
                            && was.as_deref() != Some("attendee")
                            && !r.on_stage
                            && !c.muted;
                    }
                },
                || crate::outgoing(call.0),
            );
            if mute {
                hold_the_microphone(inner, call.0, true);
            }
        }
        Event::RoomHand { call, raised } if in_room(call.0) => update_call(
            inner,
            call.0,
            |c| {
                if let Some(r) = c.room.as_mut() {
                    r.hand = *raised;
                }
            },
            || crate::outgoing(call.0),
        ),
        Event::RoomStage { call, on } if in_room(call.0) => {
            let mut attendee = false;
            update_call(
                inner,
                call.0,
                |c| {
                    if let Some(r) = c.room.as_mut() {
                        r.on_stage = *on;
                        if *on {
                            r.hand = false;
                        }
                        attendee = r.role.as_deref() == Some("attendee");
                    }
                },
                || crate::outgoing(call.0),
            );
            // Sent back to the audience: not heard again, so muted. On the
            // stage the microphone is the person's to unmute.
            if attendee && !*on {
                hold_the_microphone(inner, call.0, true);
            }
        }
        Event::RoomQuestions { call, questions } if in_room(call.0) => update_call(
            inner,
            call.0,
            |c| {
                if let Some(r) = c.room.as_mut() {
                    merge_questions(&mut r.questions, questions);
                }
            },
            || crate::outgoing(call.0),
        ),
        Event::RoomChat { call, messages } if in_room(call.0) => update_call(
            inner,
            call.0,
            |c| {
                if let Some(r) = c.room.as_mut() {
                    merge_chat(&mut r.chat, messages);
                }
            },
            || crate::outgoing(call.0),
        ),
        Event::RoomChatMuted { call, on } if in_room(call.0) => update_call(
            inner,
            call.0,
            |c| {
                if let Some(r) = c.room.as_mut() {
                    r.chat_muted = *on;
                }
            },
            || crate::outgoing(call.0),
        ),
        Event::RoomRoster { .. }
        | Event::RoomFloor { .. }
        | Event::RoomStreams { .. }
        | Event::RoomWebinar { .. }
        | Event::RoomHand { .. }
        | Event::RoomStage { .. }
        | Event::RoomQuestions { .. }
        | Event::RoomChat { .. }
        | Event::RoomChatMuted { .. } => {}
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

    /// A view as (target, height, kept first).
    fn sizes(view: &anvil_core::RoomView) -> Vec<(String, u32, bool)> {
        view.streams
            .iter()
            .flatten()
            .map(|s| {
                let who = match &s.target {
                    anvil_core::ViewTarget::Member(m) => m.clone(),
                    anvil_core::ViewTarget::Speaker => "speaker".into(),
                };
                (who, s.max_height.unwrap_or(0), s.priority == Some(1))
            })
            .collect()
    }

    #[test]
    fn a_view_asks_for_each_tile_at_its_height() {
        let m = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // A grid: as tall as a grid of that many lets a tile be.
        assert_eq!(
            sizes(&view_of(&m(&["a"]), false, None)),
            [("a".into(), 720, false)]
        );
        let four = sizes(&view_of(&m(&["a", "b", "c", "d"]), false, None));
        assert!(four.iter().all(|s| s.1 == 360 && !s.2), "{four:?}");
        let five = sizes(&view_of(&m(&["a", "b", "c", "d", "e"]), false, None));
        assert!(five.iter().all(|s| s.1 == 180), "{five:?}");
        // Pinned: large and first, the others small.
        assert_eq!(
            sizes(&view_of(&m(&["a", "b", "c"]), false, Some("b"))),
            [
                ("b".into(), 720, true),
                ("a".into(), 180, false),
                ("c".into(), 180, false)
            ]
        );
        // The speaker large; a pin wins over it.
        assert_eq!(
            sizes(&view_of(&m(&["a", "b"]), true, None)),
            [
                ("speaker".into(), 720, true),
                ("a".into(), 180, false),
                ("b".into(), 180, false)
            ]
        );
        assert_eq!(
            sizes(&view_of(&m(&["a"]), true, Some("a"))),
            [("a".into(), 720, true)]
        );
        assert_eq!(view_of(&[], false, None), anvil_core::RoomView::default());
    }

    fn question(
        id: &str,
        status: anvil_core::QuestionStatus,
        upvotes: u32,
    ) -> anvil_core::Question {
        serde_json::from_value(serde_json::json!({
            "id": id, "text": format!("Question {id}"), "asker": null, "mine": false,
            "status": match status {
                anvil_core::QuestionStatus::Pending => "pending",
                anvil_core::QuestionStatus::Answered => "answered",
                anvil_core::QuestionStatus::Dismissed => "dismissed",
            },
            "published": true, "upvotes": upvotes, "upvoted": false, "answer": null,
            "answered_live": false, "asked_at": "2026-11-21T10:00:00.000Z"
        }))
        .unwrap()
    }

    fn message(id: &str, deleted: bool) -> anvil_core::ChatMessage {
        serde_json::from_value(serde_json::json!({
            "id": id, "text": if deleted { "" } else { "hi" }, "sender": "Ann",
            "sender_id": "p-1", "sender_moderates": false, "mine": false,
            "to": "hosts", "deleted": deleted, "sent_at": "2026-11-21T10:00:00.000Z"
        }))
        .unwrap()
    }

    #[test]
    fn questions_the_room_tells_are_merged_and_dismissed_ones_dropped() {
        use anvil_core::QuestionStatus::*;
        let mut kept = Vec::new();
        merge_questions(
            &mut kept,
            &[question("q-1", Pending, 0), question("q-2", Pending, 1)],
        );
        assert_eq!(kept.len(), 2);
        merge_questions(&mut kept, &[question("q-1", Answered, 4)]);
        assert_eq!((kept[0].status.as_str(), kept[0].upvotes), ("answered", 4));
        merge_questions(
            &mut kept,
            &[question("q-2", Dismissed, 1), question("q-3", Dismissed, 0)],
        );
        assert_eq!(
            kept.iter().map(|q| q.id.as_str()).collect::<Vec<_>>(),
            ["q-1"]
        );
    }

    #[test]
    fn chat_is_merged_deleted_messages_dropped_and_the_oldest_let_go() {
        let mut kept = Vec::new();
        merge_chat(&mut kept, &[message("m-1", false), message("m-2", false)]);
        assert!(kept[0].to_hosts && kept[0].sender.as_deref() == Some("Ann"));
        merge_chat(&mut kept, &[message("m-1", true)]);
        assert_eq!(kept.len(), 1);
        let many: Vec<_> = (0..CHAT_KEPT + 10)
            .map(|i| message(&format!("x-{i}"), false))
            .collect();
        merge_chat(&mut kept, &many);
        assert_eq!(kept.len(), CHAT_KEPT);
        assert_eq!(kept.last().unwrap().id, format!("x-{}", CHAT_KEPT + 9));
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
