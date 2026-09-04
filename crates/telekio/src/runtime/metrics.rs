use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

use crate::{CallResult, Handle, abi::callback::CallbackOwner};

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum Metric {
    GlobalQueueDepth,
    NumAliveTasks,
    SpawnedTasksCount,
    BudgetForcedYieldCount,
    WorkerTotalBusyDuration,
    WorkerParkCount,
    WorkerParkUnparkCount,
    NumWorkers,
    NumBlockingThreads,
    NumIdleBlockingThreads,
    WorkerLocalQueueDepth,
    BlockingQueueDepth,
    RemoteScheduleCount,
    WorkerNoopCount,
    WorkerStealCount,
    WorkerStealOperations,
    WorkerPollCount,
    WorkerLocalScheduleCount,
    WorkerOverflowCount,
    PollTimeHistogramEnabled,
    PollTimeHistogramNumBuckets,
    PollTimeHistogramRangeStart,
    PollTimeHistogramRangeEnd,
    PollTimeHistogramBucketCount,
    WorkerMeanPollTime,
    ScheduleLatencyHistogramEnabled,
    ScheduleLatencyHistogramNumBuckets,
    ScheduleLatencyHistogramRangeStart,
    ScheduleLatencyHistogramRangeEnd,
    ScheduleLatencyHistogramBucketCount,
    IoDriverFdRegisteredCount,
    IoDriverFdDeregisteredCount,
    IoDriverReadyCount,
    CurrentWorkerIndex,
}

#[repr(C)]
pub struct MetricResult {
    pub call: CallResult,
    pub value: u64,
}

#[repr(C)]
pub struct WorkerCallback {
    owner: CallbackOwner,
    call: unsafe extern "C" fn(*const c_void, usize) -> CallResult,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct HistogramConfig {
    pub kind: u8,
    pub a: u64,
    pub b: u64,
    pub c: u64,
}

impl WorkerCallback {
    pub fn from_arc(callback: Arc<dyn Fn(usize) + Send + Sync>) -> Self {
        Self {
            owner: CallbackOwner::from_arc(callback),
            call: call_worker_callback,
        }
    }

    #[doc(hidden)]
    pub fn call(&self, worker: usize) -> CallResult {
        unsafe { (self.call)(self.owner.data(), worker) }
    }
}

impl HistogramConfig {
    #[doc(hidden)]
    pub const fn disabled() -> Self {
        Self {
            kind: 0,
            a: 0,
            b: 0,
            c: 0,
        }
    }
}

unsafe extern "C" fn call_worker_callback(data: *const c_void, worker: usize) -> CallResult {
    let callback = unsafe { &*data.cast::<Arc<dyn Fn(usize) + Send + Sync>>() };
    match catch_unwind(AssertUnwindSafe(|| callback(worker))) {
        Ok(()) => CallResult::ok(),
        Err(payload) => CallResult::panicked(&*payload),
    }
}

impl Handle {
    #[doc(hidden)]
    #[track_caller]
    pub fn metric(&self, metric: Metric, worker: usize) -> u64 {
        self.metric_bucket(metric, worker, 0)
    }

    #[doc(hidden)]
    #[track_caller]
    pub fn metric_bucket(&self, metric: Metric, worker: usize, bucket: usize) -> u64 {
        let result = unsafe { ((*self.raw.api).metric)(self.raw.context, metric, worker, bucket) };
        result.call.into_io_result().unwrap();
        result.value
    }

    #[doc(hidden)]
    pub fn observe_workers(&self, callback: WorkerCallback) -> CallResult {
        unsafe { ((*self.raw.api).observe_workers)(self.raw.context, callback) }
    }
}
