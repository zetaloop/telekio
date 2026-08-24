use super::*;
use crate::runtime::task;
use crate::runtime::task::telekio::Host;
use std::sync::Arc;

impl Handle {
    pub(crate) fn host(&self) -> &Arc<Host> {
        match self {
            Handle::CurrentThread(handle) => handle.telekio.host(),
            #[cfg(feature = "rt-multi-thread")]
            Handle::MultiThread(handle) => handle.telekio.host(),
        }
    }

    pub(crate) fn spawn_host_blocking<F, R>(&self, function: F) -> task::JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let id = task::Id::next();
        let spawned_at = task::SpawnLocation::capture();
        match self {
            Handle::CurrentThread(handle) => handle.telekio.spawn_blocking(
                Arc::clone(handle),
                function,
                id,
                spawned_at,
            ),
            #[cfg(feature = "rt-multi-thread")]
            Handle::MultiThread(handle) => handle.telekio.spawn_blocking(
                Arc::clone(handle),
                function,
                id,
                spawned_at,
            ),
        }
    }
}
