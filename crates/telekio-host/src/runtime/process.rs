use std::ffi::c_void;
#[cfg(unix)]
use std::panic::{AssertUnwindSafe, catch_unwind};

use telekio::CallResult;

#[cfg(unix)]
use crate::host_panic;

#[cfg(unix)]
#[doc(hidden)]
pub fn reap_process(id: u32) {
    tokio::runtime::telekio::reap_process(id);
}

pub(super) unsafe extern "C" fn reap_process_abi(_: *const c_void, id: u32) -> CallResult {
    #[cfg(unix)]
    return match catch_unwind(AssertUnwindSafe(|| reap_process(id))) {
        Ok(()) => CallResult::ok(),
        Err(payload) => host_panic(&*payload),
    };
    #[cfg(not(unix))]
    {
        let _ = id;
        CallResult::ok()
    }
}
