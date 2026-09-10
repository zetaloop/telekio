mod abi;
mod attachment;
mod io;
mod runtime;
mod signal;
mod time;

pub use abi::{
    BoolResult, Bytes, CallResult, Callback, OperationPoll, OwnedBytes, Poll, RuntimeApi, Status,
    StringCallback, Waker,
};
pub use attachment::{AttachResult, GuestCall, RawAttachment};
#[doc(hidden)]
pub use attachment::{attached, detach_attached, install_handle};
pub use io::{
    IO_DRIVER_DISABLED_ERROR, IoCallResult, IoDriverRegistration, IoDriverResult, IoError, IoEvent,
    IoInterest, IoKind, IoOperation, IoOperationKind, IoOperationResult, IoPoll, IoReady,
    IoRegistration, IoRequest, IoResource, IoResult,
};
pub use runtime::{
    Blocking, BlockingTask, BuildResult, DumpOperation, DumpResult, ExecutionState, Flavor, Future,
    Handle, HistogramConfig, Metric, MetricResult, NameResult, RawHandle, RawRuntime, Runtime,
    RuntimeConfig, RuntimeContext, Shutdown, SourceLocation, Task, TaskCallback, TaskEvent,
    TaskIdResult, WorkerCallback,
};
#[doc(hidden)]
pub use runtime::{execution_state, with_execution_state};
pub use signal::{Signal, SignalKind, SignalRequest, SignalResult};
pub use time::{ClockResult, ClockSample, DurationParts, InstantOffset, Timer, TimerResult};

#[doc(hidden)]
pub fn require_package(package: &str) {
    #[cfg(not(any(feature = "guest", feature = "host")))]
    {
        static WARNING: std::sync::Once = std::sync::Once::new();
        WARNING.call_once(|| {
            eprintln!(
                "{package} is running with Tokio. Install Telekio CLI and rerun the Cargo command as `telekio cargo ...`."
            );
        });
    }
    #[cfg(any(feature = "guest", feature = "host"))]
    let _ = package;
}

#[macro_export]
macro_rules! require {
    () => {
        $crate::require_package(env!("CARGO_PKG_NAME"))
    };
}

#[cfg(feature = "guest")]
#[macro_export]
macro_rules! plugin {
    () => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn telekio_attach(raw: $crate::RawHandle) -> $crate::AttachResult {
            unsafe extern "C" {
                fn telekio_guest_context(raw: $crate::RawHandle) -> $crate::AttachResult;
            }
            unsafe { telekio_guest_context(raw) }
        }
    };
}

#[cfg(not(feature = "guest"))]
#[macro_export]
macro_rules! plugin {
    () => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn telekio_attach(raw: $crate::RawHandle) -> $crate::AttachResult {
            ::core::mem::drop(unsafe { $crate::Handle::from_abi(raw) });
            $crate::require_package("This package");
            $crate::AttachResult {
                call: $crate::CallResult::error("Telekio guest is not enabled"),
                attachment: $crate::RawAttachment::empty(),
            }
        }
    };
}
