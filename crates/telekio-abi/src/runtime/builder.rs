use crate::{
    Bytes, CallResult, Callback, Flavor, Handle, HistogramConfig, OwnedBytes, RawRuntime, Runtime,
    Status, StringCallback, TaskCallback,
};

#[repr(C)]
pub struct RuntimeConfig {
    pub flavor: Flavor,
    pub enable_io: u8,
    pub enable_time: u8,
    pub start_paused: u8,
    pub worker_threads: usize,
    pub max_blocking_threads: usize,
    pub thread_name: StringCallback,
    pub thread_stack_size: usize,
    pub has_thread_stack_size: u8,
    pub after_start: Callback,
    pub before_stop: Callback,
    pub before_park: Callback,
    pub after_unpark: Callback,
    pub task_callback: TaskCallback,
    pub keep_alive_secs: u64,
    pub keep_alive_nanos: u32,
    pub has_keep_alive: u8,
    pub global_queue_interval: u32,
    pub event_interval: u32,
    pub max_io_events_per_tick: usize,
    pub rng_one: u32,
    pub rng_two: u32,
    pub name: Bytes,
    pub disable_lifo_slot: u8,
    pub eager_driver_handoff: u8,
    pub alternative_timer: u8,
    pub unhandled_panic: u8,
    pub poll_histogram: HistogramConfig,
    pub schedule_histogram: HistogramConfig,
}

#[repr(C)]
pub struct BuildResult {
    call: CallResult,
    runtime: RawRuntime,
    workers: usize,
}

impl BuildResult {
    #[doc(hidden)]
    pub fn success(runtime: RawRuntime, workers: usize) -> Self {
        assert!(!runtime.is_empty(), "host returned an empty Tokio runtime");
        Self {
            call: CallResult {
                status: Status::Ok,
                payload: OwnedBytes::empty(),
            },
            runtime,
            workers,
        }
    }

    #[doc(hidden)]
    pub fn error(call: CallResult) -> Self {
        Self {
            call,
            runtime: RawRuntime::empty(),
            workers: 0,
        }
    }

    /// # Safety
    ///
    /// A runtime built with [`Flavor::Local`] must remain on its originating
    /// thread until it has shut down.
    #[doc(hidden)]
    pub unsafe fn into_runtime(self) -> std::io::Result<(Runtime, usize)> {
        self.call.into_io_result()?;
        Ok((unsafe { Runtime::from_abi(self.runtime) }, self.workers))
    }
}

impl Handle {
    #[doc(hidden)]
    pub fn build(&self, config: RuntimeConfig) -> BuildResult {
        unsafe { ((*self.raw.api).build)(self.raw.context, config) }
    }
}
