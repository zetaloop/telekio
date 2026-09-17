use std::{
    mem::ManuallyDrop,
    panic::{catch_unwind, AssertUnwindSafe},
    ptr::NonNull,
    sync::{Arc, Mutex, Weak},
    task::Waker,
};

use crate::{future::Future as TaskFuture, runtime::task};

use super::super::{
    Header, Notified, OwnedTasks, Schedule, SpawnLocation, Task, TaskHarnessScheduleHooks,
    UnownedTask,
};
use crate::runtime::scheduler::telekio::HostSchedule;

struct TaskSchedule<S: HostSchedule> {
    schedule: S,
    runnable: Mutex<Option<Notified<Self>>>,
    waker: Mutex<Option<Waker>>,
    id: task::Id,
}

impl<S: Schedule> Task<S> {
    fn scheduler(&self) -> &S {
        unsafe { Header::get_scheduler(self.raw.header_ptr()).as_ref() }
    }
}

#[derive(Clone)]
struct BlockingSchedule<S: HostSchedule> {
    runner: Weak<BlockingRunner<S>>,
}

struct BlockingRunner<S: HostSchedule> {
    schedule: S,
    task: Mutex<Option<UnownedTask<BlockingSchedule<S>>>>,
    id: task::Id,
}

fn start<S: HostSchedule>(
    schedule: &S,
    task: Task<TaskSchedule<S>>,
    local: bool,
    location: ::telekio_abi::SourceLocation,
) {
    let id = task.scheduler().id;
    let data = task.raw.header_ptr().as_ptr().cast();
    // The host owns this task reference until it releases the descriptor.
    std::mem::forget(task);
    let task = unsafe {
        ::telekio_abi::Task::from_raw(data, poll_task::<S>, cancel_task::<S>, release_task::<S>)
    };
    let result = if local {
        schedule
            .connection()
            .handle
            .spawn_local(task, id.as_u64(), location)
    } else {
        schedule
            .connection()
            .handle
            .spawn(task, id.as_u64(), location)
    };
    if let Err(error) = result.into_io_result() {
        panic!("failed to spawn Tokio task {id}: {error}");
    }
}

fn bind<S: HostSchedule, T>(
    schedule: &S,
    future: T,
    id: task::Id,
    spawned_at: SpawnLocation,
    local: bool,
    location: ::telekio_abi::SourceLocation,
) -> task::JoinHandle<T::Output>
where
    T: TaskFuture + 'static,
    T::Output: 'static,
{
    let (task, notified, join) = super::super::new_task(
        future,
        TaskSchedule::new(schedule.clone(), id),
        id,
        spawned_at,
    );
    *task.scheduler().runnable.lock().unwrap() = Some(notified);
    start(schedule, task, local, location);
    join
}

pub(crate) fn spawn_blocking<S: HostSchedule, F, R>(
    schedule: &S,
    function: F,
    id: task::Id,
    spawned_at: task::SpawnLocation,
    location: ::telekio_abi::SourceLocation,
) -> task::JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let size = std::mem::size_of::<F>();
    let future = crate::util::trace::blocking_task::<F, _>(
        crate::runtime::blocking::BlockingTask::new(function),
        crate::util::trace::SpawnMeta::new_unnamed(size),
        id.as_u64(),
    );
    let runner = Arc::new(BlockingRunner {
        schedule: schedule.clone(),
        task: Mutex::new(None),
        id,
    });
    let task_schedule = BlockingSchedule {
        runner: Arc::downgrade(&runner),
    };
    let (task, join) = task::unowned(future, task_schedule, id, spawned_at);
    *runner.task.lock().unwrap() = Some(task);
    let task = unsafe {
        ::telekio_abi::BlockingTask::from_raw(
            Arc::into_raw(runner).cast_mut().cast(),
            run_blocking::<S>,
            cancel_blocking::<S>,
            release_blocking::<S>,
        )
    };
    if let Err(error) = schedule
        .connection()
        .handle
        .spawn_blocking(task, id.as_u64(), location)
        .into_io_result()
    {
        panic!("failed to spawn blocking Tokio task {id}: {error}");
    }
    join
}

impl<S: Schedule> Notified<S> {
    fn cancelled(&self) -> bool {
        self.0.header().state.load().is_cancelled()
    }

    fn run_host(self) {
        let raw = self.0.raw;
        std::mem::forget(self);
        raw.poll();
    }
}

impl<S: HostSchedule> OwnedTasks<S> {
    #[track_caller]
    pub(crate) fn bind_host<T>(
        &self,
        future: T,
        schedule: S,
        id: task::Id,
        spawned_at: SpawnLocation,
    ) -> (task::JoinHandle<T::Output>, Option<Notified<S>>)
    where
        T: TaskFuture + Send + 'static,
        T::Output: Send + 'static,
    {
        let join = bind(
            &schedule,
            future,
            id,
            spawned_at,
            false,
            SpawnLocation::take_telekio(),
        );
        (join, None)
    }

    #[track_caller]
    pub(crate) unsafe fn bind_local_host<T>(
        &self,
        future: T,
        schedule: S,
        id: task::Id,
        spawned_at: SpawnLocation,
    ) -> (task::JoinHandle<T::Output>, Option<Notified<S>>)
    where
        T: TaskFuture + 'static,
        T::Output: 'static,
    {
        let join = bind(
            &schedule,
            future,
            id,
            spawned_at,
            true,
            SpawnLocation::take_telekio(),
        );
        (join, None)
    }
}

impl<S: HostSchedule> Schedule for BlockingSchedule<S> {
    fn release(&self, _: &Task<Self>) -> Option<Task<Self>> {
        None
    }

    fn schedule(&self, task: Notified<Self>) {
        let cancelled = task.cancelled();
        drop(task);
        if cancelled {
            if let Some(runner) = self.runner.upgrade() {
                runner
                    .schedule
                    .connection()
                    .handle
                    .abort(runner.id.as_u64())
                    .resume("failed to abort Tokio task");
            }
        }
    }

    fn hooks(&self) -> TaskHarnessScheduleHooks {
        TaskHarnessScheduleHooks {
            task_terminate_callback: None,
        }
    }

    fn unhandled_panic(&self) {
        if let Some(runner) = self.runner.upgrade() {
            runner
                .schedule
                .connection()
                .handle
                .task_panicked()
                .resume("failed to apply Tokio panic policy");
        }
    }
}

impl<S: HostSchedule> Schedule for TaskSchedule<S> {
    fn release(&self, _: &Task<Self>) -> Option<Task<Self>> {
        let notified = self.runnable.lock().unwrap().take();
        drop(notified);
        let waker = self.waker.lock().unwrap().take();
        if let Some(waker) = waker {
            waker.wake();
        }
        None
    }

    fn schedule(&self, task: Notified<Self>) {
        let cancelled = task.cancelled();
        {
            let mut runnable = self.runnable.lock().unwrap();
            if task.0.header().state.load().is_complete() {
                return;
            }
            let previous = runnable.replace(task);
            assert!(previous.is_none(), "Tokio task was scheduled twice");
        }
        self.wake();
        if cancelled {
            self.schedule
                .connection()
                .handle
                .abort(self.id.as_u64())
                .resume("failed to abort Tokio task");
        }
    }

    fn hooks(&self) -> TaskHarnessScheduleHooks {
        TaskHarnessScheduleHooks {
            task_terminate_callback: None,
        }
    }

    fn unhandled_panic(&self) {
        self.schedule
            .connection()
            .handle
            .task_panicked()
            .resume("failed to apply Tokio panic policy");
    }
}

impl<S: HostSchedule> TaskSchedule<S> {
    fn new(schedule: S, id: task::Id) -> Self {
        Self {
            schedule,
            runnable: Mutex::new(None),
            waker: Mutex::new(None),
            id,
        }
    }

    fn wake(&self) {
        let state = ::telekio_abi::execution_state();
        if !state.is_null() && unsafe { (*state).tracing } != 0 {
            return;
        }
        if let Some(waker) = self.waker.lock().unwrap().as_ref() {
            waker.wake_by_ref();
        }
    }

    fn run(&self, runnable: Notified<Self>) {
        self.schedule.run(|| runnable.run_host())
    }

    #[cfg(feature = "taskdump")]
    fn take_or_notify(&self, task: &Task<Self>) -> Option<Notified<Self>> {
        let runnable = self
            .runnable
            .lock()
            .unwrap()
            .take()
            .or_else(|| task.notify_for_tracing());
        if runnable.is_some() {
            let waker = super::super::waker::waker_ref::<Self>(task.raw.header_ptr_ref());
            let waker = unsafe { ::telekio_abi::Waker::from_ref(&waker) };
            self.schedule
                .connection()
                .handle
                .defer(&waker)
                .resume("failed to resume traced Tokio task");
        }
        runnable
    }
}

unsafe extern "C" fn poll_task<S: HostSchedule>(
    data: *mut std::ffi::c_void,
    execution: *mut ::telekio_abi::ExecutionState,
    waker: *const ::telekio_abi::Waker,
) -> ::telekio_abi::Poll {
    match catch_unwind(AssertUnwindSafe(|| unsafe {
        ::telekio_abi::with_execution_state(execution, || poll_task_inner::<S>(data, waker))
    })) {
        Ok(poll) => poll,
        Err(_) => {
            let task = ManuallyDrop::new(unsafe {
                Task::<TaskSchedule<S>>::from_raw(NonNull::new_unchecked(data.cast()))
            });
            task.raw.remote_abort();
            ::telekio_abi::Poll::Panicked
        }
    }
}

unsafe fn poll_task_inner<S: HostSchedule>(
    data: *mut std::ffi::c_void,
    waker: *const ::telekio_abi::Waker,
) -> ::telekio_abi::Poll {
    let task = ManuallyDrop::new(unsafe {
        Task::<TaskSchedule<S>>::from_raw(NonNull::new_unchecked(data.cast()))
    });
    let runner = task.scheduler();
    if task.header().state.load().is_complete() {
        return ::telekio_abi::Poll::Ready;
    }
    let runnable = if unsafe { (*::telekio_abi::execution_state()).tracing } != 0 {
        #[cfg(feature = "taskdump")]
        {
            runner.take_or_notify(&task)
        }
        #[cfg(not(feature = "taskdump"))]
        {
            None
        }
    } else {
        runner.runnable.lock().unwrap().take()
    };
    if let Some(runnable) = runnable {
        runner.run(runnable);
    }
    if task.header().state.load().is_complete() {
        ::telekio_abi::Poll::Ready
    } else {
        *runner.waker.lock().unwrap() = Some(unsafe { (*waker).clone_rust_waker() });
        if runner.runnable.lock().unwrap().is_some() {
            runner.wake();
        }
        ::telekio_abi::Poll::Pending
    }
}

unsafe extern "C" fn cancel_task<S: HostSchedule>(
    data: *mut std::ffi::c_void,
    execution: *mut ::telekio_abi::ExecutionState,
) -> ::telekio_abi::CallResult {
    callback(|| unsafe {
        ::telekio_abi::with_execution_state(execution, || {
            let task = ManuallyDrop::new(Task::<TaskSchedule<S>>::from_raw(
                NonNull::new_unchecked(data.cast()),
            ));
            task.raw.ref_inc();
            task.scheduler().schedule.run(|| task.raw.shutdown());
        });
    })
}

unsafe extern "C" fn release_task<S: HostSchedule>(
    data: *mut std::ffi::c_void,
) -> ::telekio_abi::CallResult {
    callback(|| {
        let task =
            unsafe { Task::<TaskSchedule<S>>::from_raw(NonNull::new_unchecked(data.cast())) };
        // A rejected spawn can release the descriptor before the host polls it.
        if !task.header().state.load().is_complete() {
            let schedule = task.scheduler().schedule.clone();
            schedule.enter(|| task.shutdown());
        }
    })
}

impl<S: HostSchedule> BlockingRunner<S> {
    fn run(&self) {
        let schedule = self.schedule.clone();
        schedule.enter(|| {
            self.task
                .lock()
                .unwrap()
                .take()
                .expect("blocking task ran twice")
                .run();
        });
    }

    fn cancel(&self) {
        let schedule = self.schedule.clone();
        schedule.enter(|| {
            if let Some(task) = self.task.lock().unwrap().take() {
                task.shutdown();
            }
        });
    }
}

unsafe extern "C" fn run_blocking<S: HostSchedule>(
    data: *mut std::ffi::c_void,
    execution: *mut ::telekio_abi::ExecutionState,
) -> ::telekio_abi::CallResult {
    callback(|| unsafe {
        ::telekio_abi::with_execution_state(execution, || {
            (&*data.cast::<BlockingRunner<S>>()).run();
        });
    })
}

unsafe extern "C" fn cancel_blocking<S: HostSchedule>(
    data: *mut std::ffi::c_void,
    execution: *mut ::telekio_abi::ExecutionState,
) -> ::telekio_abi::CallResult {
    callback(|| unsafe {
        ::telekio_abi::with_execution_state(execution, || {
            (&*data.cast::<BlockingRunner<S>>()).cancel();
        });
    })
}

unsafe extern "C" fn release_blocking<S: HostSchedule>(
    data: *mut std::ffi::c_void,
) -> ::telekio_abi::CallResult {
    callback(|| drop(unsafe { Arc::from_raw(data.cast::<BlockingRunner<S>>()) }))
}

fn callback(call: impl FnOnce()) -> ::telekio_abi::CallResult {
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(()) => ::telekio_abi::CallResult::ok(),
        Err(payload) => ::telekio_abi::CallResult::panicked(&*payload),
    }
}
