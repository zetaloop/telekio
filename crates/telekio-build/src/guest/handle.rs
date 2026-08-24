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
}
