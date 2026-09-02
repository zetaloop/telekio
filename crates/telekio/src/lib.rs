mod builder;
mod io;
mod runtime;
mod signal;
mod time;

pub use builder::{BuildResult, Bytes, Callback, Flavor, RuntimeConfig, Shutdown, StringCallback};
pub use io::{
    IO_DRIVER_DISABLED_ERROR, IoCallResult, IoError, IoEvent, IoInterest, IoKind, IoOperation,
    IoOperationKind, IoOperationResult, IoPoll, IoReady, IoRegistration, IoRequest, IoResource,
    IoResult,
};
pub use runtime::telekio_guest_attach;
pub use runtime::{
    AttachResult, Blocking, BlockingTask, BoolResult, CallResult, ClockResult, Future, GuestCall,
    Handle, Metric, MetricResult, OwnedBytes, Poll, RawAttachment, RawHandle, RawRuntime, Runtime,
    RuntimeApi, Status, Task, Waker,
};
#[doc(hidden)]
pub use runtime::{attached, detach_attached, install_handle};
pub use signal::{Signal, SignalKind, SignalRequest, SignalResult};
pub use time::{ClockSample, DurationParts, InstantOffset, OperationPoll, Timer, TimerResult};

#[doc(hidden)]
pub fn require_package(package: &str) {
    #[cfg(not(feature = "guest"))]
    {
        static WARNING: std::sync::Once = std::sync::Once::new();
        WARNING.call_once(|| {
            eprintln!(
                "{package} is running with Tokio. Install Telekio CLI and rerun the Cargo command as `telekio cargo ...`."
            );
        });
    }
    #[cfg(feature = "guest")]
    let _ = package;
}

#[macro_export]
macro_rules! require {
    () => {
        $crate::require_package(env!("CARGO_PKG_NAME"))
    };
}

#[macro_export]
macro_rules! plugin {
    () => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn telekio_attach(raw: $crate::RawHandle) -> $crate::AttachResult {
            unsafe { $crate::telekio_guest_attach(raw) }
        }
    };
}
