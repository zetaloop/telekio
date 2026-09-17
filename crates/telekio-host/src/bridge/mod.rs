mod callback;
mod owner;
mod runtime;

pub use owner::{Attach, Attachment, attach};

use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
};

use telekio_abi::{CallResult, Status};

fn host_panic(payload: &(dyn Any + Send)) -> CallResult {
    let mut result = CallResult::panicked(payload);
    result.status = Status::HostPanicked;
    result
}

pub(crate) fn host_callback(call: impl FnOnce()) -> CallResult {
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(()) => CallResult::ok(),
        Err(payload) => host_panic(&*payload),
    }
}
