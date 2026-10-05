//! What the integration tests share: an audio host without devices, an
//! Anvil builder, and waiting for events.

#![allow(dead_code)] // each test file uses its own share

use std::time::Duration;

use anvil_core::audio::{AudioFormat, AudioFrame, AudioHost, AudioSink, AudioSource, DeviceInfo};
use anvil_core::config::{AccountConfig, MediaConfig};
use anvil_core::{
    Anvil, AnvilConfig, AnvilError, BrandConfig, CallId, EndReason, Event, EventStream, RegState,
    Transport,
};
use async_trait::async_trait;

pub const SOON: Duration = Duration::from_secs(5);

/// Silence at the pace a microphone gives it, and a speaker that drops it.
pub struct Quiet;

struct QuietSource {
    format: AudioFormat,
    tick: tokio::time::Interval,
}

#[async_trait]
impl AudioSource for QuietSource {
    async fn next_frame(&mut self) -> Option<AudioFrame> {
        self.tick.tick().await;
        let samples = (self.format.sample_rate * self.format.frame_ms / 1000) as usize
            * usize::from(self.format.channels);
        Some(AudioFrame {
            samples: vec![0; samples],
            format: self.format,
        })
    }
}

struct QuietSink;

#[async_trait]
impl AudioSink for QuietSink {
    async fn write_frame(&mut self, _frame: &AudioFrame) -> Result<(), AnvilError> {
        Ok(())
    }
}

impl AudioHost for Quiet {
    fn make_capture(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSource>, AnvilError> {
        Ok(Box::new(QuietSource {
            format: cfg,
            tick: tokio::time::interval(Duration::from_millis(u64::from(cfg.frame_ms))),
        }))
    }
    fn make_playback(&self, _cfg: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError> {
        Ok(Box::new(QuietSink))
    }
    fn devices(&self) -> Vec<DeviceInfo> {
        Vec::new()
    }
}

/// Anvil's own log on stderr when `ANVIL_LOG` is set (an env filter).
pub fn log() {
    if let Ok(filter) = std::env::var("ANVIL_LOG") {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_test_writer()
            .try_init();
    }
}

/// An account: `aor`, registering at `registrar` as `username`, bound to
/// `ip` (any port), asking for `expires` seconds.
pub fn account(
    aor: &str,
    registrar: &str,
    username: &str,
    password: &str,
    ip: &str,
    expires: u64,
) -> AccountConfig {
    AccountConfig {
        aor: aor.to_string(),
        registrar: registrar.to_string(),
        username: username.to_string(),
        password: password.to_string(),
        transport: Transport::Udp,
        outbound_proxy: None,
        stun: None,
        register_expires: Duration::from_secs(expires),
        user_agent: "anvil-test".to_string(),
        bind_addr: Some(format!("{ip}:0")),
        tls_extra_ca_pem: None,
        provisioning_url: None,
    }
}

pub async fn start(account: AccountConfig) -> (Anvil, EventStream) {
    log();
    Anvil::start(AnvilConfig {
        account,
        media: MediaConfig::default(),
        audio: Box::new(Quiet),
        brand: BrandConfig::default(),
    })
    .await
    .expect("anvil starts")
}

/// The first event `pick` takes within `within`.
pub async fn expect<T>(
    events: &mut EventStream,
    within: Duration,
    mut pick: impl FnMut(&Event) -> Option<T>,
) -> Option<T> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .ok()??;
        if let Some(value) = pick(&event) {
            return Some(value);
        }
    }
}

pub async fn register(anvil: &Anvil, events: &mut EventStream) {
    anvil.register().await.expect("registers");
    expect(events, SOON, |e| match e {
        Event::RegistrationChanged {
            state: RegState::Registered,
            ..
        } => Some(()),
        _ => None,
    })
    .await
    .expect("registered");
}

/// The next call that rings.
pub async fn rings(events: &mut EventStream) -> CallId {
    expect(events, SOON, |e| match e {
        Event::IncomingCall { call, .. } => Some(*call),
        _ => None,
    })
    .await
    .expect("the callee rings")
}

/// `caller` calls `target`; the callee answers; both see it up.
pub async fn answered_call(
    caller: &Anvil,
    caller_events: &mut EventStream,
    callee: &Anvil,
    callee_events: &mut EventStream,
    target: &str,
) -> (CallId, CallId) {
    let out = caller.place_call(target).await.expect("the call is placed");
    let incoming = rings(callee_events).await;
    callee.answer(incoming).await.expect("answers");
    expect(caller_events, SOON, |e| match e {
        Event::CallEstablished { call, .. } if *call == out => Some(()),
        _ => None,
    })
    .await
    .expect("the caller hears the answer");
    (out, incoming)
}

pub async fn hung_up_by_the_other_side(events: &mut EventStream, call: CallId) {
    expect(events, SOON, |e| match e {
        Event::CallEnded {
            call: c,
            reason: EndReason::RemoteHangup,
        } if *c == call => Some(()),
        _ => None,
    })
    .await
    .expect("the other side hung up");
}

/// One end for `call`, by `reason`; and no second one within a second.
pub async fn ends_once(events: &mut EventStream, call: CallId, reason: fn(&EndReason) -> bool) {
    expect(events, SOON, |e| match e {
        Event::CallEnded { call: c, reason: r } if *c == call && reason(r) => Some(()),
        _ => None,
    })
    .await
    .expect("the call ends");
    let again = expect(events, Duration::from_secs(1), |e| match e {
        Event::CallEnded { call: c, .. } if *c == call => Some(()),
        _ => None,
    })
    .await;
    assert!(again.is_none(), "the call ended twice");
}
