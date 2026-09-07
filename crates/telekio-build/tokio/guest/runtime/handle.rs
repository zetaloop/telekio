use super::*;
use std::sync::Arc;

#[cfg(all(tokio_unstable, target_has_atomic = "64"))]
use crate::runtime::metrics::telekio::Workers;

pub(crate) struct Connection {
    pub(crate) handle: ::telekio::Handle,
    #[cfg_attr(
        not(any(feature = "signal", all(unix, feature = "process"))),
        expect(dead_code)
    )]
    pub(crate) io_enabled: bool,
    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    pub(crate) workers: Arc<Workers>,
}

impl Connection {
    pub(crate) fn new(handle: ::telekio::Handle, io_enabled: bool) -> Arc<Self> {
        Arc::new(Self {
            handle,
            io_enabled,
            #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
            workers: Arc::new(Workers::new()),
        })
    }
}

cfg_taskdump! {
    pub(super) fn is_tracing() -> bool {
        let state = ::telekio::execution_state();
        !state.is_null() && unsafe { (*state).tracing } != 0
    }
}

impl Handle {
    #[cfg(feature = "test-util")]
    pub(crate) fn connection(&self) -> &Arc<Connection> {
        self.inner.connection()
    }

    /// Returns the flavor of the current runtime.
    pub fn runtime_flavor(&self) -> RuntimeFlavor {
        let _ = self.runtime_flavor_inner();
        match self.inner.connection().handle.flavor() {
            ::telekio::Flavor::CurrentThread | ::telekio::Flavor::Local => {
                RuntimeFlavor::CurrentThread
            }
            ::telekio::Flavor::MultiThread => RuntimeFlavor::MultiThread,
        }
    }
}
