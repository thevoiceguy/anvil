//! Registering where this install is woken (RFC 8599): the push address a
//! REGISTER's Contact carries, and what the registrar made of it.

use sip_core::Response;
use sip_uac::integrated::IntegratedUAC;
use tokio::sync::mpsc;

use crate::config::PushAddress;
use crate::event::{Event, PushState};

/// The push address wanted, whether the registrar refused it, and what the
/// app was last told.
pub(crate) struct Push {
    wanted: parking_lot::Mutex<Option<PushAddress>>,
    /// The registrar answered 555 to the address wanted: registrations go
    /// on without it until another is set.
    refused: parking_lot::Mutex<bool>,
    told: parking_lot::Mutex<PushState>,
    events: mpsc::Sender<Event>,
}

impl Push {
    pub(crate) fn new(wanted: Option<PushAddress>, events: mpsc::Sender<Event>) -> Self {
        Self {
            wanted: parking_lot::Mutex::new(wanted),
            refused: parking_lot::Mutex::new(false),
            told: parking_lot::Mutex::new(PushState::Off),
            events,
        }
    }

    /// A new address (or none): REGISTERs carry it from now on.
    pub(crate) async fn set(
        &self,
        uac: &IntegratedUAC,
        wanted: Option<PushAddress>,
    ) -> Result<(), crate::AnvilError> {
        let params = wanted.as_ref().map(PushAddress::contact_params);
        uac.set_register_contact_params(params.as_deref())
            .map_err(|e| crate::AnvilError::Config(format!("push address: {e}")))?;
        let off = wanted.is_none();
        *self.wanted.lock() = wanted;
        *self.refused.lock() = false;
        if off {
            self.tell(PushState::Off).await;
        }
        Ok(())
    }

    /// Whether the next REGISTER carries the address.
    pub(crate) fn sending(&self) -> bool {
        self.wanted.lock().is_some() && !*self.refused.lock()
    }

    /// A REGISTER carrying the address was answered 2xx.
    pub(crate) async fn accepted(&self) {
        self.tell(PushState::Accepted).await;
    }

    /// A REGISTER carrying the address was answered 555 Push Notification
    /// Service Not Supported (RFC 8599 §5.2): it is dropped, and the
    /// registration goes on without it.
    pub(crate) async fn refused(&self, uac: &IntegratedUAC, response: &Response) {
        let _ = uac.set_register_contact_params(None);
        *self.refused.lock() = true;
        let supported = supported_providers(response);
        tracing::warn!(
            ?supported,
            "the registrar cannot wake this app by push; registering without it"
        );
        self.tell(PushState::Refused { supported }).await;
    }

    async fn tell(&self, state: PushState) {
        {
            let mut told = self.told.lock();
            if *told == state {
                return;
            }
            *told = state.clone();
        }
        let _ = self.events.send(Event::PushChanged { state }).await;
    }
}

/// The providers a response's `Feature-Caps` says the registrar pushes
/// through (`*;+sip.pns="apns"`, RFC 8599 §13.2).
pub(crate) fn supported_providers(response: &Response) -> Vec<String> {
    let mut providers = Vec::new();
    for value in response.headers().get_all("Feature-Caps") {
        for param in value.split(';') {
            let Some((name, value)) = param.split_once('=') else {
                continue;
            };
            if name.trim().eq_ignore_ascii_case("+sip.pns") {
                let value = value.trim().trim_matches('"').to_ascii_lowercase();
                if !value.is_empty() && !providers.contains(&value) {
                    providers.push(value);
                }
            }
        }
    }
    providers
}
