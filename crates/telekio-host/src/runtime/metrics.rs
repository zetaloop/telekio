use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

use telekio::{CallResult, Flavor, Metric, MetricResult, OwnedBytes, Status, WorkerCallback};

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
        match metric {
            Metric::GlobalQueueDepth => metrics.global_queue_depth() as u64,
            Metric::NumAliveTasks => metrics.num_alive_tasks() as u64,
            #[cfg(target_has_atomic = "64")]
            Metric::SpawnedTasksCount => metrics.spawned_tasks_count(),
            #[cfg(not(target_has_atomic = "64"))]
            Metric::SpawnedTasksCount => 0,
            #[cfg(target_has_atomic = "64")]
            Metric::BudgetForcedYieldCount => metrics.budget_forced_yield_count(),
            #[cfg(not(target_has_atomic = "64"))]
            Metric::BudgetForcedYieldCount => 0,
            Metric::NumWorkers => metrics.num_workers() as u64,
            Metric::NumBlockingThreads => metrics.num_blocking_threads() as u64,
            Metric::NumIdleBlockingThreads => metrics.num_idle_blocking_threads() as u64,
            Metric::WorkerLocalQueueDepth => metrics.worker_local_queue_depth(worker) as u64,
            Metric::BlockingQueueDepth => metrics.blocking_queue_depth() as u64,
            Metric::PollTimeHistogramEnabled => metrics.poll_time_histogram_enabled().into(),
            Metric::PollTimeHistogramNumBuckets => metrics.poll_time_histogram_num_buckets() as u64,
            Metric::PollTimeHistogramRangeStart => metrics
                .poll_time_histogram_bucket_range(bucket)
                .start
                .as_nanos() as u64,
            Metric::PollTimeHistogramRangeEnd => metrics
                .poll_time_histogram_bucket_range(bucket)
                .end
                .as_nanos() as u64,
            Metric::ScheduleLatencyHistogramEnabled => {
                #[cfg(all(any(unix, windows), target_pointer_width = "64"))]
                {
                    metrics.schedule_latency_histogram_enabled().into()
                }
                #[cfg(not(all(any(unix, windows), target_pointer_width = "64")))]
                {
                    0
                }
            }
            Metric::ScheduleLatencyHistogramNumBuckets => {
                #[cfg(all(any(unix, windows), target_pointer_width = "64"))]
                {
                    metrics.schedule_latency_histogram_num_buckets() as u64
                }
                #[cfg(not(all(any(unix, windows), target_pointer_width = "64")))]
                {
                    0
                }
            }
            Metric::ScheduleLatencyHistogramRangeStart => {
                #[cfg(all(any(unix, windows), target_pointer_width = "64"))]
                {
                    metrics
                        .schedule_latency_histogram_bucket_range(bucket)
                        .start
                        .as_nanos() as u64
                }
                #[cfg(not(all(any(unix, windows), target_pointer_width = "64")))]
                {
                    0
                }
            }
            Metric::ScheduleLatencyHistogramRangeEnd => {
                #[cfg(all(any(unix, windows), target_pointer_width = "64"))]
                {
                    metrics
                        .schedule_latency_histogram_bucket_range(bucket)
                        .end
                        .as_nanos() as u64
                }
                #[cfg(not(all(any(unix, windows), target_pointer_width = "64")))]
                {
                    0
                }
            }
            Metric::CurrentWorkerIndex => tokio::runtime::worker_index()
                .map(|worker| worker as u64 + 1)
                .unwrap_or_default(),
            Metric::WorkerTotalBusyDuration => {
                #[cfg(target_has_atomic = "64")]
                {
                    metrics.worker_total_busy_duration(worker).as_nanos() as u64
                }
                #[cfg(not(target_has_atomic = "64"))]
                {
                    0
                }
            }
            Metric::WorkerParkCount => {
                #[cfg(target_has_atomic = "64")]
                {
                    metrics.worker_park_count(worker)
                }
                #[cfg(not(target_has_atomic = "64"))]
                {
                    0
                }
            }
            Metric::WorkerParkUnparkCount => {
                #[cfg(target_has_atomic = "64")]
                {
                    metrics.worker_park_unpark_count(worker)
                }
                #[cfg(not(target_has_atomic = "64"))]
                {
                    0
                }
            }
            Metric::RemoteScheduleCount => {
                #[cfg(target_has_atomic = "64")]
                {
                    metrics.remote_schedule_count()
                }
                #[cfg(not(target_has_atomic = "64"))]
                {
                    0
                }
            }
            Metric::WorkerNoopCount => {
                #[cfg(target_has_atomic = "64")]
                {
                    metrics.worker_noop_count(worker)
                }
                #[cfg(not(target_has_atomic = "64"))]
                {
                    0
                }
            }
            Metric::WorkerStealCount => {
                #[cfg(target_has_atomic = "64")]
                {
                    metrics.worker_steal_count(worker)
                }
                #[cfg(not(target_has_atomic = "64"))]
                {
                    0
                }
            }
            Metric::WorkerStealOperations => {
                #[cfg(target_has_atomic = "64")]
                {
                    metrics.worker_steal_operations(worker)
                }
                #[cfg(not(target_has_atomic = "64"))]
                {
                    0
                }
            }
            Metric::WorkerPollCount => {
                #[cfg(target_has_atomic = "64")]
                {
                    metrics.worker_poll_count(worker)
                }
                #[cfg(not(target_has_atomic = "64"))]
                {
                    0
                }
            }
            Metric::WorkerLocalScheduleCount => {
                #[cfg(target_has_atomic = "64")]
                {
                    metrics.worker_local_schedule_count(worker)
                }
                #[cfg(not(target_has_atomic = "64"))]
                {
                    0
                }
            }
            Metric::WorkerOverflowCount => {
                #[cfg(target_has_atomic = "64")]
                {
                    metrics.worker_overflow_count(worker)
                }
                #[cfg(not(target_has_atomic = "64"))]
                {
                    0
                }
            }
            Metric::PollTimeHistogramBucketCount => {
                #[cfg(target_has_atomic = "64")]
                {
                    metrics.poll_time_histogram_bucket_count(worker, bucket)
                }
                #[cfg(not(target_has_atomic = "64"))]
                {
                    0
                }
            }
            Metric::WorkerMeanPollTime => {
                #[cfg(target_has_atomic = "64")]
                {
                    metrics.worker_mean_poll_time(worker).as_nanos() as u64
                }
                #[cfg(not(target_has_atomic = "64"))]
                {
                    0
                }
            }
            Metric::ScheduleLatencyHistogramBucketCount => {
                #[cfg(all(any(unix, windows), target_pointer_width = "64"))]
                {
                    metrics.schedule_latency_histogram_bucket_count(worker, bucket)
                }
                #[cfg(not(all(any(unix, windows), target_pointer_width = "64")))]
                {
                    0
                }
            }
            Metric::IoDriverFdRegisteredCount => {
                #[cfg(all(any(unix, windows), target_has_atomic = "64"))]
                {
                    metrics.io_driver_fd_registered_count()
                }
                #[cfg(not(all(any(unix, windows), target_has_atomic = "64")))]
                {
                    0
                }
            }
            Metric::IoDriverFdDeregisteredCount => {
                #[cfg(all(any(unix, windows), target_has_atomic = "64"))]
                {
                    metrics.io_driver_fd_deregistered_count()
                }
                #[cfg(not(all(any(unix, windows), target_has_atomic = "64")))]
                {
                    0
                }
            }
            Metric::IoDriverReadyCount => {
                #[cfg(all(any(unix, windows), target_has_atomic = "64"))]
                {
                    metrics.io_driver_ready_count()
                }
                #[cfg(not(all(any(unix, windows), target_has_atomic = "64")))]
                {
                    0
                }
            }
        }
    })) {
        Ok(value) => MetricResult {
            call: result(Status::Ok, OwnedBytes::empty()),
            value,
        },
        Err(payload) => MetricResult {
            call: host_panic(&*payload),
            value: 0,
        },
    }
}

pub(super) struct WorkerObserver {
    handle: tokio::runtime::Handle,
    id: Option<u64>,
}

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
    })) {
        Ok(Ok(())) => CallResult::ok(),
        Ok(Err(error)) => result(Status::Error, OwnedBytes::from_string(error)),
        Err(payload) => host_panic(&*payload),
    }
}
