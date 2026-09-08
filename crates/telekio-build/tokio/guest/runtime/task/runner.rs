use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock, Weak,
    },
    task::Waker,
};

use crate::{future::Future as TaskFuture, runtime::task};

use super::super::{
    AbortHandle, Notified, OwnedTasks, Schedule, SpawnLocation, Task, TaskHarnessScheduleHooks,
    UnownedTask,
};
use crate::runtime::scheduler::telekio::HostSchedule;

#[derive(Clone)]
struct TaskSchedule<S: HostSchedule> {
    runner: Weak<Runner<S>>,
}

enum Runnable<S: HostSchedule> {
    Initial(UnownedTask<TaskSchedule<S>>),
    Notified(Notified<TaskSchedule<S>>),
}

struct Runner<S: HostSchedule> {
    schedule: S,
    runnable: Mutex<Option<Runnable<S>>>,
    waker: Mutex<Option<Waker>>,
    abort: OnceLock<AbortHandle>,
    complete: AtomicBool,
    polling: AtomicBool,
    id: task::Id,
    #[cfg(feature = "taskdump")]
    task: OnceLock<Task<TaskSchedule<S>>>,
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
    runner: Arc<Runner<S>>,
    local: bool,
    location: ::telekio_abi::SourceLocation,
) {
    let id = runner.id;
    let task = unsafe {
        ::telekio_abi::Task::from_raw(
            Arc::into_raw(runner).cast_mut().cast(),
            poll_runner::<S>,
            cancel_runner::<S>,
            release_runner::<S>,
        )
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
    T: TaskFuture + Send + 'static,
    T::Output: Send + 'static,
{
    let runner = Runner::new(schedule.clone(), id);
    let task_schedule = TaskSchedule {
        runner: Arc::downgrade(&runner),
    };
    let (task, join) = task::unowned(future, task_schedule, id, spawned_at);
    runner.initialize(task, &join);
    start(schedule, runner, local, location);
    join
}

unsafe fn bind_local<S: HostSchedule, T>(
    schedule: &S,
    future: T,
    id: task::Id,
    spawned_at: SpawnLocation,
    location: ::telekio_abi::SourceLocation,
) -> task::JoinHandle<T::Output>
where
    T: TaskFuture + 'static,
    T::Output: 'static,
{
    let runner = Runner::new(schedule.clone(), id);
    let task_schedule = TaskSchedule {
        runner: Arc::downgrade(&runner),
    };
    let (task, join) = unsafe { unowned_local(future, task_schedule, id, spawned_at) };
    runner.initialize(task, &join);
    start(schedule, runner, true, location);
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
        let join = unsafe {
            bind_local(
                &schedule,
                future,
                id,
                spawned_at,
                SpawnLocation::take_telekio(),
            )
        };
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
        if let Some(runner) = self.runner.upgrade() {
            runner.finish();
        }
        None
    }

    fn schedule(&self, task: Notified<Self>) {
        let cancelled = task.cancelled();
        if let Some(runner) = self.runner.upgrade() {
            runner.schedule(Runnable::Notified(task));
            if cancelled {
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

    fn yield_now(&self, task: Notified<Self>) {
        self.schedule(task);
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

unsafe fn unowned_local<T, S>(
    future: T,
    schedule: S,
    id: task::Id,
    spawned_at: SpawnLocation,
) -> (UnownedTask<S>, task::JoinHandle<T::Output>)
where
    S: Schedule,
    T: TaskFuture + 'static,
    T::Output: 'static,
{
    let (task, notified, join) = super::super::new_task(future, schedule, id, spawned_at);
    let unowned = UnownedTask {
        raw: task.raw,
        _p: std::marker::PhantomData,
    };
    std::mem::forget(task);
    std::mem::forget(notified);
    (unowned, join)
}

#[cfg(feature = "taskdump")]
fn trace_task<S>(task: &UnownedTask<S>) -> Task<S> {
    let raw = task.raw.clone();
    raw.ref_inc();
    Task {
        raw,
        _p: std::marker::PhantomData,
    }
}

impl<S: HostSchedule> Runner<S> {
    fn new(schedule: S, id: task::Id) -> Arc<Self> {
        Arc::new(Self {
            schedule,
            runnable: Mutex::new(None),
            waker: Mutex::new(None),
            abort: OnceLock::new(),
            complete: AtomicBool::new(false),
            polling: AtomicBool::new(false),
            id,
            #[cfg(feature = "taskdump")]
            task: OnceLock::new(),
        })
    }

    fn initialize<T>(&self, task: UnownedTask<TaskSchedule<S>>, join: &task::JoinHandle<T>) {
        self.abort.set(join.abort_handle()).unwrap();
        #[cfg(feature = "taskdump")]
        self.task.set(trace_task(&task)).unwrap();
        *self.runnable.lock().unwrap() = Some(Runnable::Initial(task));
    }

    fn cancel(&self) {
        self.abort.get().unwrap().abort();
        if let Some(runnable) = self.runnable.lock().unwrap().take() {
            self.run(runnable);
        }
    }

    fn schedule(&self, runnable: Runnable<S>) {
        let previous = self.runnable.lock().unwrap().replace(runnable);
        assert!(previous.is_none(), "Tokio task was scheduled twice");
        self.wake();
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

    fn run(&self, runnable: Runnable<S>) -> u64 {
        self.schedule.run(|| match runnable {
            Runnable::Initial(task) => task.run(),
            Runnable::Notified(task) => task.run_host(),
        })
    }

    #[cfg(feature = "taskdump")]
    fn take_or_notify(&self) -> Option<Runnable<S>> {
        let runnable = self.runnable.lock().unwrap().take().or_else(|| {
            self.task
                .get()
                .and_then(Task::notify_for_tracing)
                .map(Runnable::Notified)
        });
        if runnable.is_some() {
            let task = self.task.get().unwrap();
            let waker =
                super::super::waker::waker_ref::<TaskSchedule<S>>(task.raw.header_ptr_ref());
            let waker = unsafe { ::telekio_abi::Waker::from_ref(&waker) };
            self.schedule
                .connection()
                .handle
                .defer(&waker)
                .resume("failed to resume traced Tokio task");
        }
        runnable
    }

    fn finish(&self) {
        self.complete.store(true, Ordering::Release);
        if !self.polling.load(Ordering::Acquire) {
            self.wake();
        }
    }
}

unsafe extern "C" fn poll_runner<S: HostSchedule>(
    data: *mut std::ffi::c_void,
    execution: *mut ::telekio_abi::ExecutionState,
    waker: *const ::telekio_abi::Waker,
) -> ::telekio_abi::TaskPoll {
    match catch_unwind(AssertUnwindSafe(|| unsafe {
        ::telekio_abi::with_execution_state(execution, || poll_runner_inner::<S>(data, waker))
    })) {
        Ok(poll) => poll,
        Err(_) => {
            let runner = unsafe { &*data.cast::<Runner<S>>() };
            runner.abort.get().unwrap().abort();
            ::telekio_abi::TaskPoll::unmeasured(::telekio_abi::Poll::Panicked)
        }
    }
}

unsafe fn poll_runner_inner<S: HostSchedule>(
    data: *mut std::ffi::c_void,
    waker: *const ::telekio_abi::Waker,
) -> ::telekio_abi::TaskPoll {
    struct Polling<'a>(&'a AtomicBool);

    impl Drop for Polling<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }

    let runner = unsafe { &*data.cast::<Runner<S>>() };
    assert!(
        !runner.polling.swap(true, Ordering::AcqRel),
        "Tokio task is already being polled"
    );
    let _polling = Polling(&runner.polling);
    *runner.waker.lock().unwrap() = Some(unsafe { (*waker).clone_rust_waker() });
    if runner.complete.load(Ordering::Acquire) {
        return ::telekio_abi::TaskPoll::unmeasured(::telekio_abi::Poll::Ready);
    }
    let mut duration = None;
    let runnable = if unsafe { (*::telekio_abi::execution_state()).tracing } != 0 {
        #[cfg(feature = "taskdump")]
        {
            runner.take_or_notify()
        }
        #[cfg(not(feature = "taskdump"))]
        {
            None
        }
    } else {
        runner.runnable.lock().unwrap().take()
    };
    if let Some(runnable) = runnable {
        duration = Some(runner.run(runnable));
    }
    let state = if runner.complete.load(Ordering::Acquire) {
        ::telekio_abi::Poll::Ready
    } else {
        if runner.runnable.lock().unwrap().is_some() {
            runner.wake();
        }
        ::telekio_abi::Poll::Pending
    };
    duration.map_or_else(
        || ::telekio_abi::TaskPoll::unmeasured(state),
        |duration| ::telekio_abi::TaskPoll::new(state, duration),
    )
}

unsafe extern "C" fn cancel_runner<S: HostSchedule>(
    data: *mut std::ffi::c_void,
    execution: *mut ::telekio_abi::ExecutionState,
) -> ::telekio_abi::CallResult {
    callback(|| unsafe {
        ::telekio_abi::with_execution_state(execution, || {
            let runner = &*data.cast::<Runner<S>>();
            runner.cancel();
        });
    })
}

unsafe extern "C" fn release_runner<S: HostSchedule>(
    data: *mut std::ffi::c_void,
) -> ::telekio_abi::CallResult {
    callback(|| drop(unsafe { Arc::from_raw(data.cast::<Runner<S>>()) }))
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
