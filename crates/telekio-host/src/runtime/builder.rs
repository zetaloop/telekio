use std::{io, panic::resume_unwind, sync::Arc, time::Duration};

use telekio::{Flavor, RuntimeConfig, Status, StringCallback};
#[cfg(tokio_unstable)]
use telekio::{TaskCallback, TaskEvent};

use crate::owner::OwnerState;

use super::{HandleContext, LocalSlot, RuntimeKind, handle::handle_context};
use crate::callback::CallbackOwner;

#[cfg(tokio_unstable)]
struct TaskCallbackOwner(TaskCallback);

struct StringCallbackOwner(StringCallback);

#[cfg(tokio_unstable)]
impl TaskCallbackOwner {
    fn call(&self, event: TaskEvent, meta: &tokio::runtime::TaskMeta<'_>) {
        self.0
            .call(event, meta.id().telekio_value(), meta.spawned_at())
            .resume("Tokio task callback panicked");
    }
}

impl StringCallbackOwner {
    fn call(&self) -> String {
        let result = self.0.call();
        match result.status {
            Status::Ok => unsafe { result.payload.into_string() },
            Status::Panicked | Status::HostPanicked | Status::Error => {
                resume_unwind(Box::new(unsafe { result.payload.into_string() }));
            }
        }
    }
}

pub(super) fn build_runtime(
    config: RuntimeConfig,
    owner: Arc<OwnerState>,
) -> io::Result<(RuntimeKind, Arc<HandleContext>, usize)> {
    #[cfg(not(any(
        feature = "net",
        all(unix, any(feature = "process", feature = "signal")),
        all(
            tokio_unstable,
            target_os = "linux",
            feature = "fs",
            feature = "io-uring"
        )
    )))]
    if config.enable_io != 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Tokio host I/O is unavailable in this configuration",
        ));
    }
    #[cfg(not(feature = "time"))]
    if config.enable_time != 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Tokio host timers require time",
        ));
    }
    #[cfg(not(feature = "test-util"))]
    if config.start_paused != 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Tokio host paused time requires test-util",
        ));
    }
    #[cfg(not(feature = "rt-multi-thread"))]
    if config.eager_driver_handoff != 0 || config.alternative_timer != 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Tokio host threaded scheduling options require rt-multi-thread",
        ));
    }
    #[cfg(not(feature = "time"))]
    if config.alternative_timer != 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Tokio host alternative timers require time",
        ));
    }
    #[cfg(not(tokio_unstable))]
    if config.disable_lifo_slot != 0
        || config.eager_driver_handoff != 0
        || config.alternative_timer != 0
        || config.unhandled_panic != 0
        || config.poll_histogram.kind != 0
        || config.task_callback.is_some()
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Tokio host requires tokio_unstable for the requested runtime options",
        ));
    }
    #[cfg(not(all(
        tokio_unstable,
        feature = "schedule-latency",
        any(unix, windows),
        target_pointer_width = "64"
    )))]
    if config.schedule_histogram.kind != 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Tokio host does not support schedule-latency histograms in this configuration",
        ));
    }

    let thread_name = Arc::new(StringCallbackOwner(config.thread_name));
    let after_start = Arc::new(CallbackOwner(config.after_start));
    let before_stop = Arc::new(CallbackOwner(config.before_stop));
    let before_park = Arc::new(CallbackOwner(config.before_park));
    let after_unpark = Arc::new(CallbackOwner(config.after_unpark));
    #[cfg(tokio_unstable)]
    let task_callback = Arc::new(TaskCallbackOwner(config.task_callback));
    let mut builder = match config.flavor {
        Flavor::CurrentThread | Flavor::Local => tokio::runtime::Builder::new_current_thread(),
        #[cfg(feature = "rt-multi-thread")]
        Flavor::MultiThread => tokio::runtime::Builder::new_multi_thread(),
        #[cfg(not(feature = "rt-multi-thread"))]
        Flavor::MultiThread => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Tokio host multi-thread runtimes require rt-multi-thread",
            ));
        }
    };

    #[cfg(any(
        feature = "net",
        all(unix, any(feature = "process", feature = "signal")),
        all(
            tokio_unstable,
            target_os = "linux",
            feature = "fs",
            feature = "io-uring"
        )
    ))]
    if config.enable_io != 0 {
        builder.enable_io();
    }
    #[cfg(feature = "time")]
    if config.enable_time != 0 {
        builder.enable_time();
    }
    #[cfg(feature = "test-util")]
    if config.start_paused != 0 {
        builder.start_paused(true);
    }
    if config.worker_threads != 0 {
        builder.worker_threads(config.worker_threads);
    }
    builder.max_blocking_threads(config.max_blocking_threads);
    if config.has_thread_stack_size != 0 {
        builder.thread_stack_size(config.thread_stack_size);
    }
    if config.has_keep_alive != 0 {
        builder.thread_keep_alive(Duration::new(
            config.keep_alive_secs,
            config.keep_alive_nanos,
        ));
    }
    if config.global_queue_interval != 0 {
        builder.global_queue_interval(config.global_queue_interval);
    }
    builder.event_interval(config.event_interval);
    #[cfg(any(
        feature = "net",
        all(unix, any(feature = "process", feature = "signal")),
        all(
            tokio_unstable,
            target_os = "linux",
            feature = "fs",
            feature = "io-uring"
        )
    ))]
    builder.max_io_events_per_tick(config.max_io_events_per_tick);
    builder.telekio_rng_seed(config.rng_one, config.rng_two);
    #[cfg(tokio_unstable)]
    {
        if config.disable_lifo_slot != 0 {
            builder.telekio_disable_lifo_slot();
        }
        #[cfg(feature = "rt-multi-thread")]
        if config.eager_driver_handoff != 0 {
            builder.telekio_enable_eager_driver_handoff();
        }
        #[cfg(all(feature = "time", feature = "rt-multi-thread"))]
        if config.alternative_timer != 0 {
            builder.telekio_enable_alt_timer();
        }
        builder.telekio_unhandled_panic(config.unhandled_panic != 0);
        builder.telekio_poll_histogram(
            config.poll_histogram.kind,
            config.poll_histogram.a,
            config.poll_histogram.b,
            config.poll_histogram.c,
        );
    }
    #[cfg(all(
        tokio_unstable,
        feature = "schedule-latency",
        any(unix, windows),
        target_pointer_width = "64"
    ))]
    builder.telekio_schedule_histogram(
        config.schedule_histogram.kind,
        config.schedule_histogram.a,
        config.schedule_histogram.b,
        config.schedule_histogram.c,
    );
    if !config.name.is_empty() {
        builder.name(unsafe { config.name.as_str() });
    }

    builder.thread_name_fn(move || thread_name.call());
    if after_start.is_some() {
        builder.on_thread_start(move || after_start.call());
    }
    if before_stop.is_some() {
        builder.on_thread_stop(move || before_stop.call());
    }
    if before_park.is_some() {
        builder.on_thread_park(move || before_park.call());
    }
    if after_unpark.is_some() {
        builder.on_thread_unpark(move || after_unpark.call());
    }
    #[cfg(tokio_unstable)]
    if task_callback.0.is_some() {
        let callback = Arc::clone(&task_callback);
        builder.on_task_spawn(move |meta| {
            callback.call(TaskEvent::Spawn, meta);
        });
        let callback = Arc::clone(&task_callback);
        builder.on_before_task_poll(move |meta| {
            callback.call(TaskEvent::PollStart, meta);
        });
        let callback = Arc::clone(&task_callback);
        builder.on_after_task_poll(move |meta| {
            callback.call(TaskEvent::PollStop, meta);
        });
        builder.on_task_terminate(move |meta| {
            task_callback.call(TaskEvent::Terminate, meta);
        });
    }

    match config.flavor {
        Flavor::CurrentThread | Flavor::MultiThread => {
            let runtime = builder.build()?;
            let workers = runtime.handle().metrics().num_workers();
            let handle = handle_context(
                runtime.handle().clone(),
                Arc::clone(&owner),
                None,
                config.flavor,
            );
            Ok((RuntimeKind::Runtime(Some(runtime)), handle, workers))
        }
        Flavor::Local => {
            let runtime = builder.build_local(Default::default())?;
            let handle = runtime.handle().clone();
            let workers = handle.metrics().num_workers();
            let local = Arc::new(LocalSlot::new(runtime));
            let handle = handle_context(handle, owner, Some(Arc::clone(&local)), config.flavor);
            Ok((RuntimeKind::Local(local), handle, workers))
        }
    }
}
