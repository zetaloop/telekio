use super::*;

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
    pub(crate) fn install(&self, runtime: ::telekio_abi::Runtime) {
        assert!(
            self.telekio.set(runtime).is_ok(),
            "Tokio runtime was initialized twice"
        );
    }

    pub(crate) fn runtime(&self) -> &::telekio_abi::Runtime {
        self.telekio
            .get()
            .expect("Tokio runtime is not initialized")
    }

    pub(crate) fn shutdown(&mut self, timeout: Option<Duration>) {
        self.shutdown_workers(timeout);
        if let Some(runtime) = self.telekio.get() {
            let mode = if timeout.is_some() {
                ::telekio_abi::Shutdown::Timeout
            } else {
                ::telekio_abi::Shutdown::Wait
            };
            let duration = timeout.unwrap_or_default();
            runtime
                .shutdown(mode, duration.as_secs(), duration.subsec_nanos())
                .into_io_result()
                .expect("failed to shut down the Tokio runtime");
        }
    }
}
