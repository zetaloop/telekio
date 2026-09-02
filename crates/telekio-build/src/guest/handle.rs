use super::*;
#[cfg(feature = "test-util")]
use crate::runtime::task::telekio::Host;
#[cfg(feature = "test-util")]
use std::sync::Arc;

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
