use super::*;
#[cfg(not(test))]
use std::{
    ffi::c_void,
    panic::{catch_unwind, AssertUnwindSafe},
};

#[cfg(not(test))]
struct AttachedContext {
    runtime: Runtime,
    flavor: ::telekio::Flavor,
}

#[cfg(not(test))]
#[unsafe(no_mangle)]
pub(super) extern "C-unwind" fn telekio_default_handle() -> ::telekio::RawHandle {
    ::telekio::RawHandle::empty()
}

impl Builder {
    pub(super) fn build_hosted_current_thread(&mut self) -> io::Result<Runtime> {
        let host = self.build_host(false)?;
        let runtime = self.build_guest(Self::build_current_thread_runtime)?;
        runtime.install_host(host, self.enable_io);
        Ok(runtime)
    }

    pub(super) fn build_hosted_local(&mut self) -> io::Result<LocalRuntime> {
        let host = self.build_host(true)?;
        let runtime = self.build_guest(Self::build_current_thread_local_runtime)?;
        runtime.install_host(host, self.enable_io);
        Ok(runtime)
    }

    #[cfg(feature = "rt-multi-thread")]
    pub(super) fn build_hosted_multi_thread(&mut self) -> io::Result<Runtime> {
        let host = self.build_host(false)?;
        let runtime = self.build_guest(Self::build_threaded_runtime)?;
        runtime.install_host(host, self.enable_io);
        Ok(runtime)
    }

    #[cfg(not(test))]
    fn build_attached_context(&mut self, handle: ::telekio::Handle) -> io::Result<AttachedContext> {
        let flavor = handle.flavor();
        if let Some(name) = handle.name() {
            self.name(name);
        }
        self.enable_all();
        let runtime = self.build_guest(Self::build_current_thread_runtime)?;
        runtime.install_attached_host(handle);
        Ok(AttachedContext { runtime, flavor })
    }

    fn build_guest<T>(
        &mut self,
        build: impl FnOnce(&mut Self) -> io::Result<T>,
    ) -> io::Result<T> {
        let enable_io = self.enable_io;
        #[cfg(all(
            not(test),
            not(all(tokio_unstable, feature = "io-uring", target_os = "linux"))
        ))]
        {
            self.enable_io = false;
        }
        let result = build(self);
        self.enable_io = enable_io;
        result
    }

    fn build_host(&self, local: bool) -> io::Result<::telekio::Runtime> {
        let flavor = if local {
            ::telekio::Flavor::Local
        } else {
            match self.kind {
                Kind::CurrentThread => ::telekio::Flavor::CurrentThread,
                #[cfg(feature = "rt-multi-thread")]
                Kind::MultiThread => ::telekio::Flavor::MultiThread,
            }
        };
        let keep_alive = self.keep_alive.unwrap_or_default();
        let (rng_one, rng_two) = self.seed_generator.telekio_parts();
        let config = ::telekio::RuntimeConfig {
            flavor,
            enable_io: self.enable_io.into(),
            enable_time: self.enable_time.into(),
            start_paused: self.start_paused.into(),
            worker_threads: self.worker_threads.unwrap_or_default(),
            max_blocking_threads: self.max_blocking_threads,
            thread_name: ::telekio::StringCallback::from_arc(self.thread_name.clone()),
            thread_stack_size: self.thread_stack_size.unwrap_or_default(),
            has_thread_stack_size: self.thread_stack_size.is_some().into(),
            after_start: self
                .after_start
                .clone()
                .map_or_else(::telekio::Callback::none, ::telekio::Callback::from_arc),
            before_stop: self
                .before_stop
                .clone()
                .map_or_else(::telekio::Callback::none, ::telekio::Callback::from_arc),
            before_park: self
                .before_park
                .clone()
                .map_or_else(::telekio::Callback::none, ::telekio::Callback::from_arc),
            after_unpark: self
                .after_unpark
                .clone()
                .map_or_else(::telekio::Callback::none, ::telekio::Callback::from_arc),
            keep_alive_secs: keep_alive.as_secs(),
            keep_alive_nanos: keep_alive.subsec_nanos(),
            has_keep_alive: self.keep_alive.is_some().into(),
            global_queue_interval: self.global_queue_interval.unwrap_or_default(),
            event_interval: self.event_interval,
            max_io_events_per_tick: self.nevents,
            rng_one,
            rng_two,
            name: unsafe { ::telekio::Bytes::borrow(self.name.as_deref()) },
            disable_lifo_slot: self.disable_lifo_slot.into(),
            eager_driver_handoff: self.enable_eager_driver_handoff.into(),
            alternative_timer: {
                #[cfg(all(tokio_unstable, feature = "rt-multi-thread"))]
                {
                    matches!(self.timer_flavor, TimerFlavor::Alternative).into()
                }
                #[cfg(not(all(tokio_unstable, feature = "rt-multi-thread")))]
                {
                    0
                }
            },
            poll_histogram: {
                #[cfg(tokio_unstable)]
                {
                    histogram(self.metrics_poll_count_histogram_builder())
                }
                #[cfg(not(tokio_unstable))]
                {
                    ::telekio::HistogramConfig::disabled()
                }
            },
            schedule_histogram: {
                #[cfg(tokio_unstable)]
                {
                    histogram(self.metrics_schedule_latency_histogram_builder())
                }
                #[cfg(not(tokio_unstable))]
                {
                    ::telekio::HistogramConfig::disabled()
                }
            },
        };
        #[cfg(any(telekio_host, feature = "telekio-test"))]
        let result = ::telekio_host::build_root(config);
        #[cfg(not(any(telekio_host, feature = "telekio-test")))]
        let result = ::telekio::attached().build(config);
        // Tokio's LocalRuntime keeps this value on its originating thread.
        unsafe { result.into_runtime() }.map(|(runtime, _)| runtime)
    }
}

#[cfg(not(test))]
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn telekio_guest_context(
    raw: ::telekio::RawHandle,
) -> ::telekio::AttachResult {
    match catch_unwind(AssertUnwindSafe(|| {
        let handle = unsafe { ::telekio::Handle::from_abi(raw) };
        let mut builder = Builder::new_current_thread();
        let runtime = builder
            .build_attached_context(handle.clone())
            .map_err(|error| error.to_string())?;
        if let Err(handle) = ::telekio::install_handle(handle) {
            drop(handle);
            return Err("Telekio runtime is already attached".to_owned());
        }
        Ok(unsafe {
            ::telekio::RawAttachment::from_raw(
                Box::into_raw(Box::new(runtime)).cast(),
                enter_guest_context,
                detach_guest_context,
            )
        })
    })) {
        Ok(Ok(attachment)) => ::telekio::AttachResult {
            call: ::telekio::CallResult::ok(),
            attachment,
        },
        Ok(Err(error)) => ::telekio::AttachResult {
            call: ::telekio::CallResult::error(&error),
            attachment: ::telekio::RawAttachment::empty(),
        },
        Err(payload) => ::telekio::AttachResult {
            call: ::telekio::CallResult::panicked(&*payload),
            attachment: ::telekio::RawAttachment::empty(),
        },
    }
}

#[cfg(not(test))]
unsafe extern "C" fn enter_guest_context(
    data: *mut c_void,
    execution: *mut ::telekio::ExecutionState,
    call: ::telekio::GuestCall,
) -> ::telekio::CallResult {
    match catch_unwind(AssertUnwindSafe(|| unsafe {
        ::telekio::with_execution_state(execution, || {
            let context = &*data.cast::<AttachedContext>();
            context
                .runtime
                .enter_attached(context.flavor, || call.invoke())
        })
    })) {
        Ok(result) => result,
        Err(payload) => ::telekio::CallResult::panicked(&*payload),
    }
}

#[cfg(tokio_unstable)]
fn histogram(builder: Option<HistogramBuilder>) -> ::telekio::HistogramConfig {
    let Some(builder) = builder else {
        return ::telekio::HistogramConfig::disabled();
    };
    let (kind, a, b, c) = builder.telekio_parts();
    ::telekio::HistogramConfig { kind, a, b, c }
}

#[cfg(not(test))]
unsafe extern "C" fn detach_guest_context(data: *mut c_void) -> ::telekio::CallResult {
    match catch_unwind(AssertUnwindSafe(|| {
        drop(unsafe { Box::from_raw(data.cast::<AttachedContext>()) });
        unsafe { ::telekio::detach_attached() }
    })) {
        Ok(result) => result,
        Err(payload) => ::telekio::CallResult::panicked(&*payload),
    }
}
