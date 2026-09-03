use super::*;
use crate::runtime::task;
use crate::runtime::task::telekio::Host;
#[cfg(target_has_atomic = "64")]
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[cfg(target_has_atomic = "64")]
pub(crate) struct HostWorkerMetrics {
    #[cfg(tokio_unstable)]
    host: Arc<Host>,
    #[cfg(tokio_unstable)]
    worker: usize,
    pub(crate) busy_duration_total: HostMetric,
    pub(crate) park_count: HostMetric,
    pub(crate) park_unpark_count: HostMetric,
    #[cfg(tokio_unstable)]
    pub(crate) noop_count: HostMetric,
    #[cfg(tokio_unstable)]
    pub(crate) steal_count: HostMetric,
    #[cfg(tokio_unstable)]
    pub(crate) steal_operations: HostMetric,
    #[cfg(tokio_unstable)]
    pub(crate) poll_count: HostMetric,
    #[cfg(tokio_unstable)]
    pub(crate) mean_poll_time: HostMetric,
    #[cfg(tokio_unstable)]
    pub(crate) local_schedule_count: HostMetric,
    #[cfg(tokio_unstable)]
    pub(crate) overflow_count: HostMetric,
    #[cfg(tokio_unstable)]
    pub(crate) poll_count_histogram: Option<HostHistogram>,
    #[cfg(feature = "schedule-latency")]
    pub(crate) schedule_latency_histogram: Option<HostHistogram>,
}

#[cfg(all(tokio_unstable, target_has_atomic = "64"))]
pub(crate) struct HostSchedulerMetrics {
    pub(crate) remote_schedule_count: HostMetric,
}

#[cfg(all(tokio_unstable, target_has_atomic = "64"))]
pub(crate) struct HostHistogram {
    host: Arc<Host>,
    buckets: ::telekio::Metric,
    count: ::telekio::Metric,
    start: ::telekio::Metric,
    end: ::telekio::Metric,
    worker: usize,
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

#[cfg(all(tokio_unstable, target_has_atomic = "64"))]
impl HostHistogram {
    pub(crate) fn num_buckets(&self) -> usize {
        self.host.metric(self.buckets, 0) as usize
    }

    pub(crate) fn bucket_range(&self, bucket: usize) -> std::ops::Range<u64> {
        self.host.metric_bucket(self.start, 0, bucket)..self.host.metric_bucket(self.end, 0, bucket)
    }

    pub(crate) fn get(&self, bucket: usize) -> u64 {
        self.host.metric_bucket(self.count, self.worker, bucket)
    }
}

#[cfg(all(tokio_unstable, target_has_atomic = "64"))]
impl HostWorkerMetrics {
    pub(crate) fn thread_id(&self) -> Option<std::thread::ThreadId> {
        self.host.worker_thread_id(self.worker)
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

    pub(crate) fn host_num_workers(&self) -> usize {
        self.host().metric(::telekio::Metric::NumWorkers, 0) as usize
    }

    #[cfg(tokio_unstable)]
    pub(crate) fn host_num_blocking_threads(&self) -> usize {
        self.host().metric(::telekio::Metric::NumBlockingThreads, 0) as usize
    }

    #[cfg(tokio_unstable)]
    pub(crate) fn host_num_idle_blocking_threads(&self) -> usize {
        self.host()
            .metric(::telekio::Metric::NumIdleBlockingThreads, 0) as usize
    }

    #[cfg(tokio_unstable)]
    pub(crate) fn host_worker_local_queue_depth(&self, worker: usize) -> usize {
        self.host()
            .metric(::telekio::Metric::WorkerLocalQueueDepth, worker) as usize
    }

    #[cfg(tokio_unstable)]
    pub(crate) fn host_blocking_queue_depth(&self) -> usize {
        self.host().metric(::telekio::Metric::BlockingQueueDepth, 0) as usize
    }

    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    pub(crate) fn host_scheduler_metrics(&self) -> HostSchedulerMetrics {
        HostSchedulerMetrics {
            remote_schedule_count: HostMetric {
                host: Arc::clone(self.host()),
                metric: ::telekio::Metric::RemoteScheduleCount,
                worker: 0,
            },
        }
    }

    #[cfg(not(target_has_atomic = "64"))]
    pub(crate) fn host_worker_metrics(&self, worker: usize) -> &WorkerMetrics {
        self.worker_metrics(worker)
    }

    #[cfg(target_has_atomic = "64")]
    pub(crate) fn host_worker_metrics(&self, worker: usize) -> HostWorkerMetrics {
        let metric = |metric| HostMetric {
            host: Arc::clone(self.host()),
            metric,
            worker,
        };
        #[cfg(tokio_unstable)]
        let histogram = |enabled, buckets, count, start, end| {
            (self.host().metric(enabled, 0) != 0).then(|| HostHistogram {
                host: Arc::clone(self.host()),
                buckets,
                count,
                start,
                end,
                worker,
            })
        };
        HostWorkerMetrics {
            #[cfg(tokio_unstable)]
            host: Arc::clone(self.host()),
            #[cfg(tokio_unstable)]
            worker,
            busy_duration_total: metric(::telekio::Metric::WorkerTotalBusyDuration),
            park_count: metric(::telekio::Metric::WorkerParkCount),
            park_unpark_count: metric(::telekio::Metric::WorkerParkUnparkCount),
            #[cfg(tokio_unstable)]
            noop_count: metric(::telekio::Metric::WorkerNoopCount),
            #[cfg(tokio_unstable)]
            steal_count: metric(::telekio::Metric::WorkerStealCount),
            #[cfg(tokio_unstable)]
            steal_operations: metric(::telekio::Metric::WorkerStealOperations),
            #[cfg(tokio_unstable)]
            poll_count: metric(::telekio::Metric::WorkerPollCount),
            #[cfg(tokio_unstable)]
            mean_poll_time: metric(::telekio::Metric::WorkerMeanPollTime),
            #[cfg(tokio_unstable)]
            local_schedule_count: metric(::telekio::Metric::WorkerLocalScheduleCount),
            #[cfg(tokio_unstable)]
            overflow_count: metric(::telekio::Metric::WorkerOverflowCount),
            #[cfg(tokio_unstable)]
            poll_count_histogram: histogram(
                ::telekio::Metric::PollTimeHistogramEnabled,
                ::telekio::Metric::PollTimeHistogramNumBuckets,
                ::telekio::Metric::PollTimeHistogramBucketCount,
                ::telekio::Metric::PollTimeHistogramRangeStart,
                ::telekio::Metric::PollTimeHistogramRangeEnd,
            ),
            #[cfg(feature = "schedule-latency")]
            schedule_latency_histogram: histogram(
                ::telekio::Metric::ScheduleLatencyHistogramEnabled,
                ::telekio::Metric::ScheduleLatencyHistogramNumBuckets,
                ::telekio::Metric::ScheduleLatencyHistogramBucketCount,
                ::telekio::Metric::ScheduleLatencyHistogramRangeStart,
                ::telekio::Metric::ScheduleLatencyHistogramRangeEnd,
            ),
        }
    }

    #[cfg(all(tokio_unstable, feature = "net", target_has_atomic = "64"))]
    pub(crate) fn host_io_driver_metric(&self, metric: ::telekio::Metric) -> u64 {
        self.host().metric(metric, 0)
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
                    .spawn_blocking(handle.clone(), function, id, spawned_at)
            }
            #[cfg(feature = "rt-multi-thread")]
            Handle::MultiThread(handle) => {
                handle
                    .telekio
                    .spawn_blocking(handle.clone(), function, id, spawned_at)
            }
        }
    }
}
