use std::ffi::c_void;
#[cfg(all(unix, feature = "process"))]
use std::panic::{AssertUnwindSafe, catch_unwind};

use telekio_abi::CallResult;

#[cfg(all(unix, feature = "process"))]
use crate::bridge::host_panic;

pub(super) unsafe extern "C" fn reap_process_abi(_: *const c_void, id: u32) -> CallResult {
    #[cfg(all(unix, feature = "process"))]
    return match catch_unwind(AssertUnwindSafe(|| crate::runtime::telekio::reap_process(id))) {
        Ok(()) => CallResult::ok(),
        Err(payload) => host_panic(&*payload),
    };
    #[cfg(not(all(unix, feature = "process")))]
    {
        let _ = id;
        CallResult::error("Tokio host Unix process handling requires process")
    }
}
