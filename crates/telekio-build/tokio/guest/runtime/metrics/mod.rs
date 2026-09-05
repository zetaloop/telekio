#[cfg(not(target_has_atomic = "64"))]
use super::WorkerMetrics;
#[cfg(target_has_atomic = "64")]
use crate::runtime::handle::telekio::Connection;
use crate::runtime::scheduler::Handle;
#[cfg(target_has_atomic = "64")]
use std::sync::{atomic::Ordering, Arc};

#[cfg(tokio_unstable)]
mod worker;
#[cfg(tokio_unstable)]
pub(crate) use worker::worker_index;
#[cfg(all(tokio_unstable, target_has_atomic = "64"))]
pub(crate) use worker::{record_worker, root, Workers};

#[cfg(target_has_atomic = "64")]
pub(crate) struct HostWorkerMetrics {
    #[cfg(tokio_unstable)]
    connection: Arc<Connection>,
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
    pub(crate) budget_forced_yield_count: HostMetric,
}

#[cfg(all(tokio_unstable, target_has_atomic = "64"))]
pub(crate) struct HostHistogram {
    connection: Arc<Connection>,
    buckets: ::telekio::Metric,
    count: ::telekio::Metric,
    start: ::telekio::Metric,
    end: ::telekio::Metric,
    worker: usize,
}

#[cfg(target_has_atomic = "64")]
pub(crate) struct HostMetric {
    connection: Arc<Connection>,
    metric: ::telekio::Metric,
    worker: usize,
}

#[cfg(target_has_atomic = "64")]
impl HostMetric {
    pub(crate) fn load(&self, _: Ordering) -> u64 {
        self.connection.handle.metric(self.metric, self.worker)
    }
}

#[cfg(all(tokio_unstable, target_has_atomic = "64"))]
impl HostHistogram {
    pub(crate) fn num_buckets(&self) -> usize {
        self.connection.handle.metric(self.buckets, 0) as usize
    }

    pub(crate) fn bucket_range(&self, bucket: usize) -> std::ops::Range<u64> {
        self.connection.handle.metric_bucket(self.start, 0, bucket)
            ..self.connection.handle.metric_bucket(self.end, 0, bucket)
    }

    pub(crate) fn get(&self, bucket: usize) -> u64 {
        self.connection
            .handle
            .metric_bucket(self.count, self.worker, bucket)
    }
}

#[cfg(all(tokio_unstable, target_has_atomic = "64"))]
impl HostWorkerMetrics {
    pub(crate) fn thread_id(&self) -> Option<std::thread::ThreadId> {
        self.connection
            .workers
            .thread_id(&self.connection.handle, self.worker)
    }
}

impl Handle {
    pub(crate) fn host_global_queue_depth(&self) -> usize {
        self.connection()
            .handle
            .metric(::telekio::Metric::GlobalQueueDepth, 0) as usize
    }

    pub(crate) fn host_num_alive_tasks(&self) -> usize {
        self.connection()
            .handle
            .metric(::telekio::Metric::NumAliveTasks, 0) as usize
    }

    pub(crate) fn host_num_workers(&self) -> usize {
        self.connection()
            .handle
            .metric(::telekio::Metric::NumWorkers, 0) as usize
    }

    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    pub(crate) fn host_spawned_tasks_count(&self) -> u64 {
        self.connection()
            .handle
            .metric(::telekio::Metric::SpawnedTasksCount, 0)
    }

    #[cfg(tokio_unstable)]
    pub(crate) fn host_num_blocking_threads(&self) -> usize {
        self.connection()
            .handle
            .metric(::telekio::Metric::NumBlockingThreads, 0) as usize
    }

    #[cfg(tokio_unstable)]
    pub(crate) fn host_num_idle_blocking_threads(&self) -> usize {
        self.connection()
            .handle
            .metric(::telekio::Metric::NumIdleBlockingThreads, 0) as usize
    }

    #[cfg(tokio_unstable)]
    pub(crate) fn host_worker_local_queue_depth(&self, worker: usize) -> usize {
        self.connection()
            .handle
            .metric(::telekio::Metric::WorkerLocalQueueDepth, worker) as usize
    }

    #[cfg(tokio_unstable)]
    pub(crate) fn host_blocking_queue_depth(&self) -> usize {
        self.connection()
            .handle
            .metric(::telekio::Metric::BlockingQueueDepth, 0) as usize
    }

    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    pub(crate) fn host_scheduler_metrics(&self) -> HostSchedulerMetrics {
        HostSchedulerMetrics {
            remote_schedule_count: HostMetric {
                connection: Arc::clone(self.connection()),
                metric: ::telekio::Metric::RemoteScheduleCount,
                worker: 0,
            },
            budget_forced_yield_count: HostMetric {
                connection: Arc::clone(self.connection()),
                metric: ::telekio::Metric::BudgetForcedYieldCount,
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
            connection: Arc::clone(self.connection()),
            metric,
            worker,
        };
        #[cfg(tokio_unstable)]
        let histogram = |enabled, buckets, count, start, end| {
            (self.connection().handle.metric(enabled, 0) != 0).then(|| HostHistogram {
                connection: Arc::clone(self.connection()),
                buckets,
                count,
                start,
                end,
                worker,
            })
        };
        HostWorkerMetrics {
            #[cfg(tokio_unstable)]
            connection: Arc::clone(self.connection()),
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
        self.connection().handle.metric(metric, 0)
    }
}
