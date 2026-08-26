use super::*;
use crate::runtime::task;
use crate::runtime::task::telekio::Host;
use std::sync::Arc;
#[cfg(target_has_atomic = "64")]
use std::sync::atomic::Ordering;

#[cfg(target_has_atomic = "64")]
pub(crate) struct HostWorkerMetrics {
    pub(crate) busy_duration_total: HostMetric,
    pub(crate) park_count: HostMetric,
    pub(crate) park_unpark_count: HostMetric,
}

#[cfg(target_has_atomic = "64")]
pub(crate) struct HostMetric {
    host: Arc<Host>,
    metric: ::telekio::Metric,
    worker: usize,
}

#[cfg(target_has_atomic = "64")]
impl HostMetric {
    pub(crate) fn load(&self, _: Ordering) -> u64 {
        self.host.metric(self.metric, self.worker)
    }
}

impl Handle {
    pub(crate) fn host(&self) -> &Arc<Host> {
        match self {
            Handle::CurrentThread(handle) => handle.telekio.host(),
            #[cfg(feature = "rt-multi-thread")]
            Handle::MultiThread(handle) => handle.telekio.host(),
        }
    }

    pub(crate) fn host_global_queue_depth(&self) -> usize {
        self.host().metric(::telekio::Metric::GlobalQueueDepth, 0) as usize
    }

    #[cfg(target_has_atomic = "64")]
    pub(crate) fn host_worker_metrics(&self, worker: usize) -> HostWorkerMetrics {
        let metric = |metric| HostMetric {
            host: Arc::clone(self.host()),
            metric,
            worker,
        };
        HostWorkerMetrics {
            busy_duration_total: metric(::telekio::Metric::WorkerTotalBusyDuration),
            park_count: metric(::telekio::Metric::WorkerParkCount),
            park_unpark_count: metric(::telekio::Metric::WorkerParkUnparkCount),
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
            Handle::CurrentThread(handle) => {
                handle
                    .telekio
                    .spawn_blocking(Arc::clone(handle), function, id, spawned_at)
            }
            #[cfg(feature = "rt-multi-thread")]
            Handle::MultiThread(handle) => {
                handle
                    .telekio
                    .spawn_blocking(Arc::clone(handle), function, id, spawned_at)
            }
        }
    }
}
