cfg_io_driver! {
    pub use crate::io::{interest::Interest, ready::Ready};
    #[cfg(target_os = "linux")]
    pub use super::handle::telekio::TelekioIo;
    #[cfg(all(unix, feature = "net"))]
    pub use crate::io::unix::AsyncFd;
    #[cfg(all(unix, not(feature = "net")))]
    pub use crate::runtime::io::async_fd::AsyncFd;
}

cfg_aio! {
    pub use super::handle::telekio::TelekioAio;
}

#[cfg(all(unix, any(feature = "signal", feature = "process")))]
pub use crate::signal::unix::{signal, Signal, SignalKind};

#[cfg(feature = "rt")]
pub fn next_task_id() -> u64 {
    super::task::Id::next_local().telekio_value()
}

#[cfg(feature = "rt")]
pub fn with_task<R>(
    id: u64,
    location: Option<&'static std::panic::Location<'static>>,
    call: impl FnOnce() -> R,
) -> R {
    crate::util::trace::telekio::forward(|| super::task::Id::with_telekio(id, location, call))
}

pub fn defer(waker: &std::task::Waker) {
    super::context::defer(waker);
}

pub fn record_forced_yields(count: u64) {
    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    if count != 0 {
        _ = super::context::with_current(|handle| {
            for _ in 0..count {
                handle.scheduler_metrics().inc_budget_forced_yield_count();
            }
        });
    }
    #[cfg(not(all(tokio_unstable, target_has_atomic = "64")))]
    let _ = count;
}

cfg_taskdump! {
    pub fn is_tracing() -> bool {
        super::task::trace::telekio::is_tracing()
    }

    pub fn trace_leaf(root: *const std::ffi::c_void, leaf: *const std::ffi::c_void) {
        super::task::trace::telekio::trace_leaf(root, leaf);
    }
}

cfg_not_taskdump! {
    pub fn is_tracing() -> bool {
        false
    }

    pub fn trace_leaf(_: *const std::ffi::c_void, _: *const std::ffi::c_void) {}
}

#[cfg(feature = "rt")]
pub fn with_execution<R>(call: impl FnOnce(*mut std::ffi::c_void) -> R) -> R {
    super::context::telekio::with(call)
}

pub fn with_task_execution<R>(call: impl FnOnce(*mut std::ffi::c_void) -> R) -> R {
    super::context::telekio::with_task(call)
}

#[cfg(all(unix, feature = "process"))]
pub fn reap_process(id: u32) {
    crate::process::unix::telekio::push(id);
}
