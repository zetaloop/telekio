use super::Schedule;
use crate::runtime::task;
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

mod hooks;
mod observability;
mod runner;

pub(crate) use hooks::HostTaskHooks;
pub(crate) use observability::measure_poll;
#[cfg(all(tokio_unstable, target_has_atomic = "64"))]
use observability::Workers;

pub(crate) trait HostSchedule: Schedule + Clone + Send + Sync + 'static {
    fn registry(&self) -> &Registry<Self>;
    fn run(&self, call: impl FnOnce()) -> u64;
    fn enter<R>(&self, call: impl FnOnce() -> R) -> R;
}

macro_rules! host_schedule {
    ($variant:ident, $other:ident, $block_in_place:literal) => {
        impl Handle {
            pub(crate) fn owned_id(&self) -> std::num::NonZeroU64 {
                let _ = self.owned_id_inner();
                std::num::NonZeroU64::new(self.telekio.host().id()).expect("invalid runtime ID")
            }

            pub(crate) fn install(
                &self,
                host: std::sync::Arc<crate::runtime::task::telekio::Host>,
            ) {
                self.telekio.install(host);
            }
        }

        impl crate::runtime::task::telekio::HostSchedule for std::sync::Arc<Handle> {
            fn registry(&self) -> &crate::runtime::task::telekio::Registry<Self> {
                &self.telekio
            }

            fn run(&self, call: impl FnOnce()) -> u64 {
                let run = || {
                    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
                    self.telekio.host().record_worker();
                    crate::runtime::task::telekio::measure_poll(call)
                };
                let current = crate::runtime::context::with_current(|handle| match handle {
                    crate::runtime::scheduler::Handle::$variant(handle) => {
                        std::sync::Arc::ptr_eq(handle, self)
                    }
                    #[cfg(feature = "rt-multi-thread")]
                    crate::runtime::scheduler::Handle::$other(_) => false,
                })
                .unwrap_or(false);
                if current {
                    crate::runtime::context::telekio::enter(run)
                } else {
                    let handle =
                        crate::runtime::scheduler::Handle::$variant(std::sync::Arc::clone(self));
                    crate::runtime::context::enter_runtime(&handle, $block_in_place, |_| {
                        crate::runtime::context::telekio::enter(run)
                    })
                }
            }

            fn enter<R>(&self, call: impl FnOnce() -> R) -> R {
                let handle =
                    crate::runtime::scheduler::Handle::$variant(std::sync::Arc::clone(self));
                let _guard = crate::runtime::context::try_set_current(&handle);
                call()
            }
        }
    };
}

pub(crate) use host_schedule;

pub(crate) struct Host {
    runtime: Option<::telekio::Runtime>,
    handle: ::telekio::Handle,
    #[cfg_attr(
        not(any(feature = "signal", all(unix, feature = "process"))),
        expect(dead_code)
    )]
    io_enabled: bool,
    task_hooks: Arc<HostTaskHooks>,
    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    workers: Arc<Workers>,
    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    observing: OnceLock<()>,
}

pub(crate) struct Registry<S: HostSchedule> {
    host: OnceLock<Arc<Host>>,
    marker: std::marker::PhantomData<fn() -> S>,
}

impl Host {
    pub(crate) fn new(
        runtime: ::telekio::Runtime,
        io_enabled: bool,
        task_hooks: Arc<HostTaskHooks>,
    ) -> Arc<Self> {
        Arc::new(Self {
            handle: runtime.handle(),
            runtime: Some(runtime),
            io_enabled,
            task_hooks,
            #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
            workers: Arc::new(Workers::new()),
            #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
            observing: OnceLock::new(),
        })
    }

    #[cfg(not(test))]
    pub(crate) fn attached(handle: ::telekio::Handle) -> Arc<Self> {
        let host = Arc::new(Self {
            runtime: None,
            handle,
            io_enabled: true,
            task_hooks: HostTaskHooks::empty(),
            #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
            workers: Arc::new(Workers::new()),
            #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
            observing: OnceLock::new(),
        });
        host
    }

    pub(crate) fn unhandled_panic(&self) {
        self.handle
            .task_panicked()
            .resume("failed to apply Tokio panic policy");
    }

    pub(crate) fn abort(&self, id: task::Id) {
        self.handle
            .abort(id.as_u64())
            .resume("failed to abort Tokio task");
    }

    pub(crate) fn defer(&self, waker: &::telekio::Waker) -> ::telekio::CallResult {
        self.handle.defer(waker)
    }

    pub(crate) fn flavor(&self) -> ::telekio::Flavor {
        self.handle.flavor()
    }

    pub(crate) fn id(&self) -> u64 {
        self.handle.id()
    }

    pub(crate) fn runtime_block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        #[cfg(tokio_unstable)]
        let future = self.root(future);
        match &self.runtime {
            Some(runtime) => runtime.block_on(future),
            None => self.handle.block_on(future),
        }
    }

    pub(crate) fn handle_block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        #[cfg(tokio_unstable)]
        let future = self.root(future);
        self.handle.block_on(future)
    }

    #[cfg(feature = "rt-multi-thread")]
    pub(crate) fn block_in_place(&self, blocking: ::telekio::Blocking) -> ::telekio::CallResult {
        self.handle.block_in_place(blocking)
    }

    #[cfg(feature = "test-util")]
    pub(crate) fn now(&self) -> std::time::Instant {
        self.handle.now()
    }

    #[cfg(feature = "test-util")]
    pub(crate) fn pause(&self) {
        self.handle.pause().into_io_result().unwrap();
    }

    #[cfg(feature = "test-util")]
    pub(crate) fn resume(&self) {
        self.handle.resume().into_io_result().unwrap();
    }

    #[cfg(feature = "test-util")]
    pub(crate) fn advance(&self, duration: Duration) {
        self.handle.advance(duration).into_io_result().unwrap();
    }

    #[cfg(feature = "time")]
    pub(crate) fn timer(&self, deadline: std::time::Instant) -> ::telekio::Timer {
        ::telekio::Timer::from_result(self.handle.timer(deadline))
    }

    #[cfg(any(feature = "signal", all(unix, feature = "process")))]
    #[cfg_attr(test, expect(dead_code))]
    #[track_caller]
    pub(crate) fn signal(
        &self,
        request: ::telekio::SignalRequest,
    ) -> std::io::Result<::telekio::Signal> {
        assert!(
            self.io_enabled,
            "there is no signal driver running, must be called from the context of Tokio runtime"
        );
        self.handle.signal(request)
    }

    #[cfg(all(
        tokio_unstable,
        feature = "io-uring",
        feature = "rt",
        feature = "fs",
        target_os = "linux"
    ))]
    pub(crate) fn register_io_driver(
        &self,
        resource: ::telekio::IoResource,
        callback: ::telekio::Callback,
    ) -> ::telekio::IoDriverResult {
        self.handle.register_io_driver(resource, callback)
    }

    #[cfg(any(
        feature = "net",
        all(unix, any(feature = "process", feature = "signal")),
        all(unix, feature = "rt", feature = "fs", feature = "io-uring")
    ))]
    #[track_caller]
    pub(crate) fn register_io(
        &self,
        resource: ::telekio::IoResource,
        interest: ::telekio::IoInterest,
    ) -> std::io::Result<::telekio::IoRegistration> {
        self.handle.register_io(resource, interest)
    }

    pub(crate) fn shutdown(&self, mode: ::telekio::Shutdown, duration: Option<Duration>) {
        let duration = duration.unwrap_or_default();
        if let Some(runtime) = &self.runtime {
            runtime
                .shutdown(mode, duration.as_secs(), duration.subsec_nanos())
                .into_io_result()
                .expect("failed to shut down the Tokio runtime");
        }
    }
}
