//! `AnvilHandle` — opaque pointer the FFI hands back to C callers.
//!
//! Bundles the `Anvil` runtime with its dedicated Tokio executor and a
//! background task draining the event stream. C callers see only an
//! opaque `*mut AnvilHandle`.

use std::sync::Arc;

use anvil_core::{Anvil, AnvilConfig, AnvilError, EventStream};
use tokio::runtime::Runtime;

/// Opaque to C; the inner type is `pub` so other modules in this crate
/// can drive it.
pub struct AnvilHandle {
    runtime: Arc<Runtime>,
    inner: Option<Anvil>,
    _events: tokio::task::JoinHandle<()>,
}

impl AnvilHandle {
    pub(crate) fn start(cfg: AnvilConfig) -> Result<Self, AnvilError> {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("anvil-ffi-rt")
                .build()
                .map_err(|e| AnvilError::Internal(format!("tokio runtime: {e}")))?,
        );

        let (anvil, events) = runtime.block_on(Anvil::start(cfg))?;

        // Drain events into the void for now. P3-M2 will replace this
        // with a forwarder that calls the user-registered C callback.
        let drain = runtime.spawn(drain_events(events));

        Ok(Self {
            runtime,
            inner: Some(anvil),
            _events: drain,
        })
    }

    /// Run a future to completion on the FFI runtime.
    pub(crate) fn block_on<F: std::future::Future>(&self, fut: F) -> F::Output {
        self.runtime.block_on(fut)
    }

    /// Borrow the inner `Anvil`. Panics if shutdown has already taken it,
    /// which can't happen via the public C ABI but is worth asserting in
    /// case future code paths expose it.
    pub(crate) fn anvil(&self) -> &Anvil {
        self.inner
            .as_ref()
            .expect("Anvil already taken by shutdown")
    }

    /// Tear the runtime down. Called from the `Drop` glue in `anvil_destroy`.
    pub(crate) fn shutdown(mut self) {
        if let Some(anvil) = self.inner.take() {
            // Best-effort unregister + shutdown. Errors here are logged and
            // swallowed because the destroy path can't return them.
            self.runtime.block_on(async move {
                let _ = anvil.unregister().await;
                let _ = anvil.shutdown().await;
            });
        }
    }
}

async fn drain_events(mut events: EventStream) {
    while let Some(event) = events.recv().await {
        tracing::trace!(?event, "ffi event drain");
    }
}
