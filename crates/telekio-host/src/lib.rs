#[path = "dump.rs"]
mod host_dump;
#[path = "io.rs"]
mod host_io;
#[path = "signal.rs"]
mod host_signal;
mod location;
mod metrics;
mod owner;
mod runtime;
mod task;
mod time;

pub use owner::{Attach, Attachment, Owner};
#[cfg(unix)]
pub use runtime::reap_process;
pub use runtime::{Runtime, build_root, next_task_id};

use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
};

use telekio::{CallResult, OwnedBytes, RuntimeApi, Status};

static RUNTIME_API: RuntimeApi = RuntimeApi {
    runtime_block_on: task::runtime_block_on,
    handle_block_on: task::handle_block_on,
    retain_handle: owner::retain_handle,
    release_handle: owner::release_handle,
    release_runtime: runtime::release_runtime,
    detach: owner::detach,
    task_id: runtime::task_id,
    abort: task::abort,
    spawn: task::spawn,
    spawn_local: task::spawn_local,
    spawn_blocking: task::spawn_blocking,
    task_panicked: task::panicked,
    trace_leaf: runtime::trace_leaf,
    dump: host_dump::start,
    block_in_place: runtime::block_in_place,
    build: runtime::build,
    clock: time::clock,
    pause: time::pause,
    resume: time::resume,
    advance: time::advance,
    timer: time::timer,
    register_io: host_io::register,
    register_io_driver: host_io::register_driver,
    signal: host_signal::signal,
    reap_process: runtime::reap_process_abi,
    shutdown: runtime::shutdown,
    defer: runtime::defer,
    metric: metrics::metric,
    flavor: runtime::flavor,
    id: runtime::id,
    name: runtime::name,
    observe_workers: runtime::observe_workers,
};

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
