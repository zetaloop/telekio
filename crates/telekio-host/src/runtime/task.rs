#[cfg(feature = "rt-multi-thread")]
use std::time::Instant;
use std::{
    ffi::c_void,
    future::Future as RustFuture,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll as RustPoll},
};

use telekio::{CallResult, OwnedBytes, Poll, SourceLocation, Status, Task, TaskIdResult, Waker};

use crate::owner::{OwnerContext, TaskCleanup};
use crate::{host_panic, result};

use super::{HandleContext, context::with_task_execution};

#[doc(hidden)]
pub fn next_task_id() -> u64 {
    tokio::runtime::telekio::next_task_id()
}

pub(super) unsafe extern "C" fn task_id(_: *const c_void) -> TaskIdResult {
    match catch_unwind(next_task_id) {
        Ok(value) => TaskIdResult {
            call: CallResult::ok(),
            value,
        },
        Err(payload) => TaskIdResult {
            call: host_panic(&*payload),
            value: 0,
        },
    }
}

pub(super) unsafe extern "C" fn abort(context: *const c_void, id: u64) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| context.owner.abort_task(id))) {
        Ok(()) => CallResult::ok(),
        Err(payload) => host_panic(&*payload),
    }
}

pub(super) unsafe extern "C" fn panicked(context: *const c_void) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| {
        context.handle.telekio_unhandled_panic();
    })) {
        Ok(()) => CallResult::ok(),
        Err(payload) => host_panic(&*payload),
    }
}

pub(super) unsafe extern "C" fn spawn(
    context: *const c_void,
    task: Task,
    id: u64,
    location: SourceLocation,
) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| {
        let location = super::location::intern(location);
        let task = GuestTask::new(task);
        context.owner.reserve_task(id)?;
        let task = TrackedTask {
            task,
            cleanup: TaskCleanup {
                owner: Arc::clone(&context.owner),
                id,
            },
        };
        let handle = context.handle.telekio_spawn(task, id, location);
        context.owner.register_task(id, handle.abort_handle());
        drop(handle);
        Ok::<_, String>(())
    })) {
        Ok(Ok(())) => result(Status::Ok, OwnedBytes::empty()),
        Ok(Err(error)) => result(Status::Error, OwnedBytes::from_string(error)),
        Err(payload) => host_panic(&*payload),
    }
}

pub(super) unsafe extern "C" fn spawn_local(
    context: *const c_void,
    task: Task,
    id: u64,
    location: SourceLocation,
) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    let spawned = catch_unwind(AssertUnwindSafe(|| {
        let location = super::location::intern(location);
        let task = GuestTask::new(task);
        context.owner.reserve_task(id)?;
        let task = TrackedTask {
            task,
            cleanup: TaskCleanup {
                owner: Arc::clone(&context.owner),
                id,
            },
        };
        let local = context
            .local
            .as_ref()
            .ok_or_else(|| "spawn_local requires a LocalRuntime".to_owned())?;
        let handle = local
            .with(|runtime| unsafe { runtime.handle().telekio_spawn_local(task, id, location) })?;
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

struct GuestTask {
    task: Task,
    complete: bool,
}

struct TrackedTask {
    task: GuestTask,
    cleanup: TaskCleanup,
}

unsafe impl Send for TrackedTask {}

impl GuestTask {
    fn new(task: Task) -> Self {
        Self {
            task,
            complete: false,
        }
    }
}

impl RustFuture for GuestTask {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut TaskContext<'_>) -> RustPoll<()> {
        let waker = unsafe { Waker::from_ref(context.waker()) };
        #[cfg(feature = "rt-multi-thread")]
        let started = Instant::now();
        let poll = with_task_execution(|state| self.task.poll(unsafe { &mut *state }, &waker));
        #[cfg(feature = "rt-multi-thread")]
        if let Some(guest) = poll.duration_nanos() {
            let actual = started.elapsed().as_nanos().min(u64::MAX.into()) as u64;
            tokio::runtime::Handle::telekio_record_poll(actual, guest);
        }
        match poll.state() {
            Poll::Pending => RustPoll::Pending,
            Poll::Ready => {
                self.complete = true;
                RustPoll::Ready(())
            }
            Poll::Panicked => RustPoll::Ready(()),
        }
    }
}

impl RustFuture for TrackedTask {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut TaskContext<'_>) -> RustPoll<()> {
        let _owner = OwnerContext::enter(&self.cleanup.owner);
        if !self.cleanup.owner.is_accepting() {
            return RustPoll::Ready(());
        }
        Pin::new(&mut self.task).poll(context)
    }
}

impl Drop for GuestTask {
    fn drop(&mut self) {
        if !self.complete {
            with_task_execution(|state| unsafe { self.task.cancel(state) });
        }
    }
}
