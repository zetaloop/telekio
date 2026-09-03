use std::{
    any::Any,
    ffi::c_void,
    future::Future as RustFuture,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll as RustPoll},
    time::Instant,
};

use telekio::{BlockingTask, CallResult, Future, OwnedBytes, Poll, Status, Task, Waker};

use super::{
    Activity, HandleContext, OwnerContext, OwnerState, RuntimeKind, RuntimeOwner, TaskCleanup,
    host_panic, result,
};

pub(super) unsafe extern "C" fn runtime_block_on(owner: *mut c_void, future: Future) -> CallResult {
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<Status, String> {
        let runtime = unsafe { &*owner.cast::<RuntimeOwner>() };
        let owner = runtime
            .owner
            .upgrade()
            .ok_or_else(|| "Tokio owner has gone away".to_owned())?;
        let activity = owner.activity()?;
        let kind = runtime.kind.read().unwrap();
        let status = match &*kind {
            RuntimeKind::Runtime(runtime) => runtime
                .as_ref()
                .expect("Tokio runtime has shut down")
                .block_on(GuestFuture::new(future, &activity)),
            RuntimeKind::Local(runtime) => runtime
                .with(|runtime| runtime.block_on(GuestFuture::new(future, &activity)))
                .unwrap_or_else(|error| panic!("{error}")),
            RuntimeKind::Closed => panic!("Tokio runtime has shut down"),
        };
        drop(activity);
        Ok(status)
    }));
    match outcome {
        Ok(Ok(status)) => block_on_result(Ok(status)),
        Ok(Err(error)) => result(Status::Error, OwnedBytes::from_string(error)),
        Err(payload) => host_panic(&*payload),
    }
}

pub(super) unsafe extern "C" fn handle_block_on(
    context: *const c_void,
    future: Future,
) -> CallResult {
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<Status, String> {
        let context = unsafe { &*context.cast::<HandleContext>() };
        let activity = context.owner.activity()?;
        let status = context.handle.block_on(GuestFuture::new(future, &activity));
        drop(activity);
        Ok(status)
    }));
    match outcome {
        Ok(Ok(status)) => block_on_result(Ok(status)),
        Ok(Err(error)) => result(Status::Error, OwnedBytes::from_string(error)),
        Err(payload) => host_panic(&*payload),
    }
}

fn block_on_result(outcome: Result<Status, Box<dyn Any + Send>>) -> CallResult {
    match outcome {
        Ok(Status::Error) => result(
            Status::Error,
            OwnedBytes::from_string("Tokio owner is shutting down".to_owned()),
        ),
        Ok(status) => result(status, OwnedBytes::empty()),
        Err(payload) => host_panic(&*payload),
    }
}

pub(super) unsafe extern "C" fn spawn(context: *const c_void, task: Task) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| {
        let task = GuestTask::new(task);
        let tracking_id = context.owner.reserve_task()?;
        let task = TrackedTask {
            task,
            cleanup: TaskCleanup {
                owner: Arc::clone(&context.owner),
                id: tracking_id,
            },
        };
        let handle = context.handle.spawn(task);
        context
            .owner
            .register_task(tracking_id, handle.abort_handle());
        drop(handle);
        Ok::<_, String>(())
    })) {
        Ok(Ok(())) => result(Status::Ok, OwnedBytes::empty()),
        Ok(Err(error)) => result(Status::Error, OwnedBytes::from_string(error)),
        Err(payload) => host_panic(&*payload),
    }
}

pub(super) unsafe extern "C" fn spawn_local(context: *const c_void, task: Task) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    let spawned = catch_unwind(AssertUnwindSafe(|| {
        let task = GuestTask::new(task);
        let tracking_id = context.owner.reserve_task()?;
        let task = TrackedTask {
            task,
            cleanup: TaskCleanup {
                owner: Arc::clone(&context.owner),
                id: tracking_id,
            },
        };
        let local = context
            .local
            .as_ref()
            .ok_or_else(|| "spawn_local requires a LocalRuntime".to_owned())?;
        let handle = local.with(|runtime| runtime.spawn_local(task))?;
        context
            .owner
            .register_task(tracking_id, handle.abort_handle());
        drop(handle);
        Ok::<_, String>(())
    }));
    match spawned {
        Ok(Ok(())) => result(Status::Ok, OwnedBytes::empty()),
        Ok(Err(error)) => result(Status::Error, OwnedBytes::from_string(error)),
        Err(payload) => host_panic(&*payload),
    }
}

pub(super) unsafe extern "C" fn spawn_blocking(
    context: *const c_void,
    task: BlockingTask,
) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    let spawned = catch_unwind(AssertUnwindSafe(|| {
        let task = GuestBlockingTask::new(task);
        let tracking_id = context.owner.reserve_task()?;
        let task = TrackedBlockingTask {
            task,
            cleanup: TaskCleanup {
                owner: Arc::clone(&context.owner),
                id: tracking_id,
            },
        };
        let handle = context.handle.spawn_blocking(move || task.run());
        context
            .owner
            .register_task(tracking_id, handle.abort_handle());
        drop(handle);
        Ok::<_, String>(())
    }));
    match spawned {
        Ok(Ok(())) => result(Status::Ok, OwnedBytes::empty()),
        Ok(Err(error)) => result(Status::Error, OwnedBytes::from_string(error)),
        Err(payload) => host_panic(&*payload),
    }
}

struct GuestFuture {
    future: Future,
    owner: Arc<OwnerState>,
    activity: u64,
}

impl GuestFuture {
    fn new(future: Future, activity: &Activity) -> Self {
        Self {
            future,
            owner: Arc::clone(&activity.owner),
            activity: activity.id,
        }
    }
}

impl RustFuture for GuestFuture {
    type Output = Status;

    fn poll(mut self: Pin<&mut Self>, context: &mut TaskContext<'_>) -> RustPoll<Self::Output> {
        if !self
            .owner
            .update_activity_waker(self.activity, context.waker())
        {
            return RustPoll::Ready(Status::Error);
        }
        let waker = unsafe { Waker::from_ref(context.waker()) };
        match self.future.poll(&waker) {
            Poll::Pending => RustPoll::Pending,
            Poll::Ready => RustPoll::Ready(Status::Ok),
            Poll::Panicked => RustPoll::Ready(Status::Panicked),
        }
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
        let started = Instant::now();
        let poll = self.task.poll(&waker);
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
            unsafe { self.task.cancel() };
        }
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
        unsafe { self.task.run() };
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
            unsafe { self.task.cancel() };
        }
    }
}
