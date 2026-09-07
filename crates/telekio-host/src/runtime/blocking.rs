use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

use telekio::{Blocking, BlockingTask, CallResult, OwnedBytes, SourceLocation, Status};

use crate::owner::{OwnerContext, TaskCleanup};
use crate::{host_panic, result};

use super::{HandleContext, context::with_task_execution};

#[cfg(feature = "rt-multi-thread")]
pub(super) unsafe extern "C" fn block_in_place(
    context: *const c_void,
    blocking: Blocking,
) -> CallResult {
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<Status, String> {
        let context = unsafe { &*context.cast::<HandleContext>() };
        let activity = context.owner.activity()?;
        context.owner.wake_activities();
        let status = tokio::task::block_in_place(|| unsafe { blocking.run() });
        drop(activity);
        Ok(status)
    }));
    match outcome {
        Ok(Ok(status)) => result(status, OwnedBytes::empty()),
        Ok(Err(error)) => result(Status::Error, OwnedBytes::from_string(error)),
        Err(payload) => host_panic(&*payload),
    }
}

#[cfg(not(feature = "rt-multi-thread"))]
pub(super) unsafe extern "C" fn block_in_place(_: *const c_void, _: Blocking) -> CallResult {
    CallResult::error("Tokio host block_in_place requires rt-multi-thread")
}

pub(super) unsafe extern "C" fn spawn_blocking(
    context: *const c_void,
    task: BlockingTask,
    id: u64,
    location: SourceLocation,
) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    let spawned = catch_unwind(AssertUnwindSafe(|| {
        let location = super::location::intern(location);
        let task = GuestBlockingTask::new(task);
        context.owner.reserve_task(id)?;
        let task = TrackedBlockingTask {
            task,
            cleanup: TaskCleanup {
                owner: Arc::clone(&context.owner),
                id,
            },
        };
        let handle = context
            .handle
            .telekio_spawn_blocking(move || task.run(), id, location);
        context.owner.register_task(id, handle.abort_handle());
        drop(handle);
        Ok::<_, String>(())
    }));
    match spawned {
        Ok(Ok(())) => result(Status::Ok, OwnedBytes::empty()),
        Ok(Err(error)) => result(Status::Error, OwnedBytes::from_string(error)),
        Err(payload) => host_panic(&*payload),
    }
}

struct GuestBlockingTask {
    task: BlockingTask,
    complete: bool,
}

struct TrackedBlockingTask {
    task: GuestBlockingTask,
    cleanup: TaskCleanup,
}

impl GuestBlockingTask {
    fn new(task: BlockingTask) -> Self {
        Self {
            task,
            complete: false,
        }
    }

    fn run(&mut self) {
        with_task_execution(|state| unsafe { self.task.run(state) });
        self.complete = true;
    }
}

impl TrackedBlockingTask {
    fn run(mut self) {
        let _owner = OwnerContext::enter(&self.cleanup.owner);
        if self.cleanup.owner.is_accepting() {
            self.task.run();
        }
    }
}

impl Drop for GuestBlockingTask {
    fn drop(&mut self) {
        if !self.complete {
            with_task_execution(|state| unsafe { self.task.cancel(state) });
        }
    }
}
