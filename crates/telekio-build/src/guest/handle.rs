use super::*;
#[cfg(feature = "test-util")]
use crate::runtime::task::telekio::Host;
#[cfg(feature = "test-util")]
use std::sync::Arc;

cfg_taskdump! {
    pub(super) async fn dump<F>(
        handle: &Handle,
        original: F,
    ) -> crate::runtime::Dump
    where
        F: std::future::Future<Output = crate::runtime::Dump>,
    {
        let _ = original;
        handle.inner.host().dump().await
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
    pub(crate) fn host(&self) -> &Arc<Host> {
        self.inner.host()
    }

    /// Returns the flavor of the current runtime.
    pub fn runtime_flavor(&self) -> RuntimeFlavor {
        let _ = self.runtime_flavor_inner();
        match self.inner.host().flavor() {
            ::telekio::Flavor::CurrentThread | ::telekio::Flavor::Local => {
                RuntimeFlavor::CurrentThread
            }
            ::telekio::Flavor::MultiThread => RuntimeFlavor::MultiThread,
        }
    }
}
