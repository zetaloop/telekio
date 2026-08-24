use super::*;
use crate::runtime::task::telekio::Host;
use std::sync::Arc;

impl Spawner {
    pub(crate) fn spawn_host_blocking<F, R>(&self, runtime: &Handle, function: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        runtime.inner.spawn_host_blocking(function)
    }
}

impl BlockingPool {
    pub(crate) fn install(&self, host: Arc<Host>) {
        assert!(
            self.telekio.set(host).is_ok(),
            "Tokio runtime was initialized twice"
        );
    }

    pub(crate) fn shutdown(&mut self, timeout: Option<Duration>) {
        self.shutdown_workers(timeout);
        if let Some(host) = self.telekio.get() {
            let mode = if timeout.is_some() {
                ::telekio::Shutdown::Timeout
            } else {
                ::telekio::Shutdown::Wait
            };
            host.shutdown(mode, timeout);
        }
    }
}
