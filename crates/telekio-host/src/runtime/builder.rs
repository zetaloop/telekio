use std::{io, panic::resume_unwind, sync::Arc, time::Duration};

use telekio::{Flavor, RuntimeConfig, Status, StringCallback, TaskCallback, TaskEvent};

use crate::owner::OwnerState;

use super::{HandleContext, LocalSlot, RuntimeKind, handle::handle_context};
use crate::callback::CallbackOwner;

struct TaskCallbackOwner(TaskCallback);

struct StringCallbackOwner(StringCallback);

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
    let thread_name = Arc::new(StringCallbackOwner(config.thread_name));
    let after_start = Arc::new(CallbackOwner(config.after_start));
    let before_stop = Arc::new(CallbackOwner(config.before_stop));
    let before_park = Arc::new(CallbackOwner(config.before_park));
    let after_unpark = Arc::new(CallbackOwner(config.after_unpark));
    let task_callback = Arc::new(TaskCallbackOwner(config.task_callback));
    let mut builder = match config.flavor {
        Flavor::CurrentThread | Flavor::Local => tokio::runtime::Builder::new_current_thread(),
        Flavor::MultiThread => tokio::runtime::Builder::new_multi_thread(),
    };

    #[cfg(any(unix, windows))]
    if config.enable_io != 0 {
        builder.enable_io();
    }
    if config.enable_time != 0 {
        builder.enable_time();
    }
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
    #[cfg(any(unix, windows))]
    builder.max_io_events_per_tick(config.max_io_events_per_tick);
    builder.telekio_rng_seed(config.rng_one, config.rng_two);
    if config.disable_lifo_slot != 0 {
        builder.telekio_disable_lifo_slot();
    }
    if config.eager_driver_handoff != 0 {
        builder.telekio_enable_eager_driver_handoff();
    }
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
                config.enable_io != 0,
                config.flavor,
            );
            Ok((RuntimeKind::Runtime(Some(runtime)), handle, workers))
        }
        Flavor::Local => {
            let runtime = builder.build_local(Default::default())?;
            let handle = runtime.handle().clone();
            let workers = handle.metrics().num_workers();
            let local = Arc::new(LocalSlot::new(runtime));
            let handle = handle_context(
                handle,
                owner,
                Some(Arc::clone(&local)),
                config.enable_io != 0,
                config.flavor,
            );
            Ok((RuntimeKind::Local(local), handle, workers))
        }
    }
}
