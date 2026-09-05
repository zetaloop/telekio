mod callback;
mod owner;
mod runtime;

pub use owner::{Attach, Attachment, Owner};
#[cfg(unix)]
pub use runtime::reap_process;
pub use runtime::{Runtime, build_root, next_task_id};

use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
};

use telekio::{CallResult, OwnedBytes, Status};

fn result(status: Status, payload: OwnedBytes) -> CallResult {
    CallResult { status, payload }
}

fn host_panic(payload: &(dyn Any + Send)) -> CallResult {
    result(
        Status::HostPanicked,
        OwnedBytes::from_string(panic_message(payload)),
    )
}

pub(crate) fn host_callback(call: impl FnOnce()) -> CallResult {
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(()) => CallResult::ok(),
        Err(payload) => host_panic(&*payload),
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&'static str>()
                .map(|message| (*message).to_owned())
        })
        .unwrap_or_else(|| "Box<dyn Any>".to_owned())
}
