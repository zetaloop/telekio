use super::*;

impl RuntimeMetrics {
    #[cfg(all(tokio_unstable, feature = "net", target_has_atomic = "64"))]
    pub(super) fn host_io_driver_registered<F>(&self, _: F) -> u64
    where
        F: Fn(&super::super::IoDriverMetrics) -> u64,
    {
        self.handle
            .inner
            .host_io_driver_metric(::telekio_abi::Metric::IoDriverFdRegisteredCount)
    }

    #[cfg(all(tokio_unstable, feature = "net", target_has_atomic = "64"))]
    pub(super) fn host_io_driver_deregistered<F>(&self, _: F) -> u64
    where
        F: Fn(&super::super::IoDriverMetrics) -> u64,
    {
        self.handle
            .inner
            .host_io_driver_metric(::telekio_abi::Metric::IoDriverFdDeregisteredCount)
    }

    #[cfg(all(tokio_unstable, feature = "net", target_has_atomic = "64"))]
    pub(super) fn host_io_driver_ready<F>(&self, _: F) -> u64
    where
        F: Fn(&super::super::IoDriverMetrics) -> u64,
    {
        self.handle
            .inner
            .host_io_driver_metric(::telekio_abi::Metric::IoDriverReadyCount)
    }
}
