#[cfg(tokio_unstable)]
use std::sync::Arc;
use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
};

#[cfg(tokio_unstable)]
use telekio::Flavor;
use telekio::{CallResult, Metric, MetricResult, OwnedBytes, Status, WorkerCallback};

use crate::{host_panic, result};

use super::HandleContext;

pub(super) unsafe extern "C" fn metric(
    context: *const c_void,
    metric: Metric,
    worker: usize,
    bucket: usize,
) -> MetricResult {
    match catch_unwind(AssertUnwindSafe(|| {
        let context = unsafe { &*context.cast::<HandleContext>() };
        let metrics = context.handle.metrics();
        #[cfg(not(target_has_atomic = "64"))]
        let _ = (worker, bucket);
        #[cfg(not(tokio_unstable))]
        let _ = bucket;
        let value = match metric {
            Metric::GlobalQueueDepth => metrics.global_queue_depth() as u64,
            Metric::NumAliveTasks => metrics.num_alive_tasks() as u64,
            Metric::NumWorkers => metrics.num_workers() as u64,
            #[cfg(target_has_atomic = "64")]
            Metric::WorkerTotalBusyDuration => {
                metrics.worker_total_busy_duration(worker).as_nanos() as u64
            }
            #[cfg(target_has_atomic = "64")]
            Metric::WorkerParkCount => metrics.worker_park_count(worker),
            #[cfg(target_has_atomic = "64")]
            Metric::WorkerParkUnparkCount => metrics.worker_park_unpark_count(worker),
            _ => {
                #[cfg(tokio_unstable)]
                {
                    match metric {
                        #[cfg(target_has_atomic = "64")]
                        Metric::SpawnedTasksCount => metrics.spawned_tasks_count(),
                        #[cfg(target_has_atomic = "64")]
                        Metric::BudgetForcedYieldCount => metrics.budget_forced_yield_count(),
                        Metric::NumBlockingThreads => metrics.num_blocking_threads() as u64,
                        Metric::NumIdleBlockingThreads => {
                            metrics.num_idle_blocking_threads() as u64
                        }
                        Metric::WorkerLocalQueueDepth => {
                            metrics.worker_local_queue_depth(worker) as u64
                        }
                        Metric::BlockingQueueDepth => metrics.blocking_queue_depth() as u64,
                        Metric::PollTimeHistogramEnabled => {
                            metrics.poll_time_histogram_enabled().into()
                        }
                        Metric::PollTimeHistogramNumBuckets => {
                            metrics.poll_time_histogram_num_buckets() as u64
                        }
                        Metric::PollTimeHistogramRangeStart => metrics
                            .poll_time_histogram_bucket_range(bucket)
                            .start
                            .as_nanos()
                            as u64,
                        Metric::PollTimeHistogramRangeEnd => metrics
                            .poll_time_histogram_bucket_range(bucket)
                            .end
                            .as_nanos()
                            as u64,
                        Metric::CurrentWorkerIndex => tokio::runtime::worker_index()
                            .map(|worker| worker as u64 + 1)
                            .unwrap_or_default(),
                        #[cfg(target_has_atomic = "64")]
                        Metric::RemoteScheduleCount => metrics.remote_schedule_count(),
                        #[cfg(target_has_atomic = "64")]
                        Metric::WorkerNoopCount => metrics.worker_noop_count(worker),
                        #[cfg(target_has_atomic = "64")]
                        Metric::WorkerStealCount => metrics.worker_steal_count(worker),
                        #[cfg(target_has_atomic = "64")]
                        Metric::WorkerStealOperations => metrics.worker_steal_operations(worker),
                        #[cfg(target_has_atomic = "64")]
                        Metric::WorkerPollCount => metrics.worker_poll_count(worker),
                        #[cfg(target_has_atomic = "64")]
                        Metric::WorkerLocalScheduleCount => {
                            metrics.worker_local_schedule_count(worker)
                        }
                        #[cfg(target_has_atomic = "64")]
                        Metric::WorkerOverflowCount => metrics.worker_overflow_count(worker),
                        #[cfg(target_has_atomic = "64")]
                        Metric::PollTimeHistogramBucketCount => {
                            metrics.poll_time_histogram_bucket_count(worker, bucket)
                        }
                        #[cfg(target_has_atomic = "64")]
                        Metric::WorkerMeanPollTime => {
                            metrics.worker_mean_poll_time(worker).as_nanos() as u64
                        }
                        #[cfg(all(feature = "net", any(unix, windows), target_has_atomic = "64"))]
                        Metric::IoDriverFdRegisteredCount => {
                            metrics.io_driver_fd_registered_count()
                        }
                        #[cfg(all(feature = "net", any(unix, windows), target_has_atomic = "64"))]
                        Metric::IoDriverFdDeregisteredCount => {
                            metrics.io_driver_fd_deregistered_count()
                        }
                        #[cfg(all(feature = "net", any(unix, windows), target_has_atomic = "64"))]
                        Metric::IoDriverReadyCount => metrics.io_driver_ready_count(),
                        #[cfg(all(
                            feature = "schedule-latency",
                            any(unix, windows),
                            target_pointer_width = "64"
                        ))]
                        Metric::ScheduleLatencyHistogramEnabled => {
                            metrics.schedule_latency_histogram_enabled().into()
                        }
                        #[cfg(all(
                            feature = "schedule-latency",
                            any(unix, windows),
                            target_pointer_width = "64"
                        ))]
                        Metric::ScheduleLatencyHistogramNumBuckets => {
                            metrics.schedule_latency_histogram_num_buckets() as u64
                        }
                        #[cfg(all(
                            feature = "schedule-latency",
                            any(unix, windows),
                            target_pointer_width = "64"
                        ))]
                        Metric::ScheduleLatencyHistogramRangeStart => metrics
                            .schedule_latency_histogram_bucket_range(bucket)
                            .start
                            .as_nanos()
                            as u64,
                        #[cfg(all(
                            feature = "schedule-latency",
                            any(unix, windows),
                            target_pointer_width = "64"
                        ))]
                        Metric::ScheduleLatencyHistogramRangeEnd => metrics
                            .schedule_latency_histogram_bucket_range(bucket)
                            .end
                            .as_nanos()
                            as u64,
                        #[cfg(all(
                            feature = "schedule-latency",
                            any(unix, windows),
                            target_pointer_width = "64"
                        ))]
                        Metric::ScheduleLatencyHistogramBucketCount => {
                            metrics.schedule_latency_histogram_bucket_count(worker, bucket)
                        }
                        _ => return Err("Tokio host metric is unavailable in this configuration"),
                    }
                }
                #[cfg(not(tokio_unstable))]
                return Err("Tokio host metric requires tokio_unstable");
            }
        };
        Ok(value)
    })) {
        Ok(Ok(value)) => MetricResult {
            call: result(Status::Ok, OwnedBytes::empty()),
            value,
        },
        Ok(Err(error)) => MetricResult {
            call: CallResult::error(error),
            value: 0,
        },
        Err(payload) => MetricResult {
            call: host_panic(&*payload),
            value: 0,
        },
    }
}

#[cfg(tokio_unstable)]
pub(super) struct WorkerObserver {
    handle: tokio::runtime::Handle,
    id: Option<u64>,
}

#[cfg(tokio_unstable)]
impl Drop for WorkerObserver {
    fn drop(&mut self) {
        self.handle.telekio_remove_worker_observer(self.id);
    }
}

pub(super) unsafe extern "C" fn observe_workers(
    context: *const c_void,
    callback: WorkerCallback,
) -> CallResult {
    match catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
        #[cfg(not(tokio_unstable))]
        {
            let _ = context;
            drop(callback);
            Err("Tokio host worker observations require tokio_unstable".to_owned())
        }
        #[cfg(tokio_unstable)]
        {
            let context = unsafe { &*context.cast::<HandleContext>() };
            let _activity = context.owner.activity()?;
            let mut observer = context.worker_observer.lock().unwrap();
            if observer.is_some() {
                return Err("Tokio worker observer is already installed".to_owned());
            }
            if !matches!(context.flavor, Flavor::MultiThread) {
                drop(callback);
                return Ok(());
            }
            let callback = Arc::new(callback);
            let owner = Arc::clone(&context.owner);
            let id = context
                .handle
                .telekio_add_worker_observer(Arc::new(move |worker| {
                    let Some(_activity) = owner.callback(std::task::Waker::noop().clone()) else {
                        return;
                    };
                    callback
                        .call(worker)
                        .resume("failed to record Tokio worker thread");
                }));
            *observer = Some(WorkerObserver {
                handle: context.handle.clone(),
                id,
            });
            Ok(())
        }
    })) {
        Ok(Ok(())) => CallResult::ok(),
        Ok(Err(error)) => result(Status::Error, OwnedBytes::from_string(error)),
        Err(payload) => host_panic(&*payload),
    }
}
