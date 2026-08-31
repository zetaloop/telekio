use super::{AbortHandle, Header, Notified, OwnedTasks, Schedule, Task, UnownedTask};
use crate::runtime::task;
use std::{
    collections::HashMap,
    future::Future,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};

pub(crate) trait HostSchedule: Schedule + Clone + Send + Sync + 'static {
    fn registry(&self) -> &Registry<Self>;
    fn run(&self, task: Notified<Self>);
    fn enter<R>(&self, call: impl FnOnce() -> R) -> R;
}

pub(crate) struct Host {
    runtime: ::telekio::Runtime,
    handle: ::telekio::Handle,
}

pub(crate) struct Registry<S: HostSchedule> {
    host: OnceLock<Arc<Host>>,
    runners: Mutex<HashMap<task::Id, Arc<Runner<S>>>>,
}

struct Runner<S: HostSchedule> {
    notified: Mutex<Option<Notified<S>>>,
    waker: Mutex<Option<Waker>>,
    abort: AbortHandle,
    complete: AtomicBool,
}

struct RunnerTask<S: HostSchedule> {
    schedule: S,
    runner: Arc<Runner<S>>,
}

struct BlockingRunner<S: HostSchedule> {
    schedule: S,
    task: Option<UnownedTask<S>>,
}

struct Budgeted<F>(F);

pub(crate) fn budget<F: Future>(future: F) -> impl Future<Output = F::Output> {
    Budgeted(future)
}

impl<F: Future> Future for Budgeted<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let future = unsafe { self.map_unchecked_mut(|budgeted| &mut budgeted.0) };
        crate::task::coop::budget(|| future.poll(context))
    }
}

impl Host {
    pub(crate) fn new(runtime: ::telekio::Runtime) -> Arc<Self> {
        Arc::new(Self {
            handle: runtime.handle(),
            runtime,
        })
    }

    pub(crate) fn defer(&self, waker: &::telekio::Waker) -> ::telekio::CallResult {
        self.handle.defer(waker)
    }

    pub(crate) fn metric(&self, metric: ::telekio::Metric, worker: usize) -> u64 {
        self.handle.metric(metric, worker)
    }

    pub(crate) fn runtime_block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }

    pub(crate) fn handle_block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        self.handle.block_on(future)
    }

    #[cfg(feature = "rt-multi-thread")]
    pub(crate) fn block_in_place(&self, blocking: ::telekio::Blocking) -> ::telekio::CallResult {
        self.handle.block_in_place(blocking)
    }

    pub(crate) fn now(&self) -> std::time::Instant {
        self.handle.now()
    }

    #[cfg(feature = "test-util")]
    pub(crate) fn pause(&self) {
        self.handle.pause().into_io_result().unwrap();
    }

    #[cfg(feature = "test-util")]
    pub(crate) fn resume(&self) {
        self.handle.resume().into_io_result().unwrap();
    }

    #[cfg(feature = "test-util")]
    pub(crate) fn advance(&self, duration: Duration) {
        self.handle.advance(duration).into_io_result().unwrap();
    }

    pub(crate) fn timer(&self, duration: Duration) -> ::telekio::Timer {
        ::telekio::Timer::from_result(self.handle.timer(duration))
    }

    #[cfg(feature = "signal")]
    #[cfg_attr(test, expect(dead_code))]
    pub(crate) fn signal(
        &self,
        request: ::telekio::SignalRequest,
    ) -> std::io::Result<::telekio::Signal> {
        self.handle.signal(request)
    }

    #[cfg(any(feature = "net", feature = "process", feature = "signal"))]
    #[track_caller]
    pub(crate) fn register_io(
        &self,
        resource: ::telekio::IoResource,
        interest: ::telekio::IoInterest,
    ) -> std::io::Result<::telekio::IoRegistration> {
        self.handle.register_io(resource, interest)
    }

    pub(crate) fn shutdown(&self, mode: ::telekio::Shutdown, duration: Option<Duration>) {
        let duration = duration.unwrap_or_default();
        self.runtime
            .shutdown(mode, duration.as_secs(), duration.subsec_nanos())
            .into_io_result()
            .expect("failed to shut down the Tokio runtime");
    }
}

impl<S: HostSchedule> Registry<S> {
    pub(crate) fn new() -> Self {
        Self {
            host: OnceLock::new(),
            runners: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn install(&self, host: Arc<Host>) {
        assert!(
            self.host.set(host).is_ok(),
            "Tokio runtime was initialized twice"
        );
    }

    pub(crate) fn host(&self) -> &Arc<Host> {
        self.host.get().expect("Tokio runtime is not initialized")
    }

    pub(crate) fn schedule(&self, schedule: S, notified: Notified<S>, local: bool) {
        let id = notified.id();
        let mut runners = self.runners.lock().unwrap();
        if let Some(runner) = runners.get(&id) {
            runner.schedule(notified);
            return;
        }

        let runner = Arc::new(Runner {
            abort: notified.abort_handle(),
            notified: Mutex::new(Some(notified)),
            waker: Mutex::new(None),
            complete: AtomicBool::new(false),
        });
        runners.insert(id, Arc::clone(&runner));
        drop(runners);

        let task = Box::new(RunnerTask { schedule, runner });
        let task = unsafe {
            ::telekio::Task::from_raw(
                Box::into_raw(task).cast(),
                poll_runner::<S>,
                cancel_runner::<S>,
                release_runner::<S>,
            )
        };
        let result = if local {
            self.host().handle.spawn_local(task)
        } else {
            self.host().handle.spawn(task)
        };
        result
            .into_io_result()
            .unwrap_or_else(|error| panic!("failed to spawn Tokio task {id}: {error}"));
    }

    pub(crate) fn release(&self, task: &Task<S>) {
        if let Some(runner) = self.runners.lock().unwrap().remove(&task.id()) {
            runner.finish();
        }
    }

    pub(crate) fn spawn_blocking<F, R>(
        &self,
        schedule: S,
        function: F,
        id: task::Id,
        spawned_at: task::SpawnLocation,
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
        let runner_schedule = schedule.clone();
        let (task, join) = task::unowned(future, schedule, id, spawned_at);
        let runner = Box::new(BlockingRunner {
            schedule: runner_schedule,
            task: Some(task),
        });
        let task = unsafe {
            ::telekio::BlockingTask::from_raw(
                Box::into_raw(runner).cast(),
                run_blocking::<S>,
                cancel_blocking::<S>,
                release_blocking::<S>,
            )
        };
        self.host()
            .handle
            .spawn_blocking(task)
            .into_io_result()
            .unwrap_or_else(|error| panic!("failed to spawn blocking Tokio task {id}: {error}"));
        join
    }
}

impl<S: HostSchedule> Notified<S> {
    pub(crate) fn schedule_host(self, local: bool) {
        let schedule = self.schedule();
        schedule.registry().schedule(schedule.clone(), self, local);
    }

    fn abort_handle(&self) -> AbortHandle {
        let raw = self.0.raw.clone();
        raw.ref_inc();
        AbortHandle::new(raw)
    }

    fn schedule(&self) -> S {
        unsafe { Header::get_scheduler::<S>(self.0.header_ptr()).as_ref() }.clone()
    }

    fn id(&self) -> task::Id {
        unsafe { Header::get_id(self.0.header_ptr()) }
    }
}

impl<S: HostSchedule> Task<S> {
    fn id(&self) -> task::Id {
        unsafe { Header::get_id(self.header_ptr()) }
    }

    fn schedule(&self) -> S {
        unsafe { Header::get_scheduler::<S>(self.header_ptr()).as_ref() }.clone()
    }
}

impl<S: HostSchedule> OwnedTasks<S> {
    pub(crate) fn remove_host_task(&self, task: &Task<S>) -> Option<Task<S>> {
        task.schedule().registry().release(task);
        self.remove(task)
    }
}

impl<S: HostSchedule> Runner<S> {
    fn schedule(&self, notified: Notified<S>) {
        let previous = self.notified.lock().unwrap().replace(notified);
        assert!(previous.is_none(), "Tokio task was scheduled twice");
        self.wake();
    }

    fn wake(&self) {
        if let Some(waker) = self.waker.lock().unwrap().as_ref() {
            waker.wake_by_ref();
        }
    }

    fn finish(&self) {
        self.complete.store(true, Ordering::Release);
        self.wake();
    }
}

unsafe extern "C" fn poll_runner<S: HostSchedule>(
    data: *mut std::ffi::c_void,
    waker: *const ::telekio::Waker,
) -> ::telekio::Poll {
    match catch_unwind(AssertUnwindSafe(|| unsafe {
        poll_runner_inner::<S>(data, waker)
    })) {
        Ok(poll) => poll,
        Err(_) => {
            let task = unsafe { &mut *data.cast::<RunnerTask<S>>() };
            task.runner.abort.abort();
            ::telekio::Poll::Panicked
        }
    }
}

unsafe fn poll_runner_inner<S: HostSchedule>(
    data: *mut std::ffi::c_void,
    waker: *const ::telekio::Waker,
) -> ::telekio::Poll {
    let task = unsafe { &mut *data.cast::<RunnerTask<S>>() };
    *task.runner.waker.lock().unwrap() = Some(unsafe { (*waker).clone_rust_waker() });
    if task.runner.complete.load(Ordering::Acquire) {
        return ::telekio::Poll::Ready;
    }
    let notified = task.runner.notified.lock().unwrap().take();
    if let Some(notified) = notified {
        task.schedule.run(notified);
    }
    if task.runner.complete.load(Ordering::Acquire) {
        ::telekio::Poll::Ready
    } else {
        if task.runner.notified.lock().unwrap().is_some() {
            task.runner.wake();
        }
        ::telekio::Poll::Pending
    }
}

unsafe extern "C" fn cancel_runner<S: HostSchedule>(
    data: *mut std::ffi::c_void,
) -> ::telekio::CallResult {
    callback(|| {
        let task = unsafe { &mut *data.cast::<RunnerTask<S>>() };
        task.runner.abort.abort();
        let notified = task.runner.notified.lock().unwrap().take();
        if let Some(notified) = notified {
            task.schedule.run(notified);
        }
    })
}

unsafe extern "C" fn release_runner<S: HostSchedule>(
    data: *mut std::ffi::c_void,
) -> ::telekio::CallResult {
    callback(|| drop(unsafe { Box::from_raw(data.cast::<RunnerTask<S>>()) }))
}

unsafe extern "C" fn run_blocking<S: HostSchedule>(
    data: *mut std::ffi::c_void,
) -> ::telekio::CallResult {
    callback(|| {
        let runner = unsafe { &mut *data.cast::<BlockingRunner<S>>() };
        let schedule = runner.schedule.clone();
        schedule.enter(|| runner.task.take().expect("blocking task ran twice").run());
    })
}

unsafe extern "C" fn cancel_blocking<S: HostSchedule>(
    data: *mut std::ffi::c_void,
) -> ::telekio::CallResult {
    callback(|| {
        let runner = unsafe { &mut *data.cast::<BlockingRunner<S>>() };
        let schedule = runner.schedule.clone();
        schedule.enter(|| {
            if let Some(task) = runner.task.take() {
                task.shutdown();
            }
        });
    })
}

unsafe extern "C" fn release_blocking<S: HostSchedule>(
    data: *mut std::ffi::c_void,
) -> ::telekio::CallResult {
    callback(|| drop(unsafe { Box::from_raw(data.cast::<BlockingRunner<S>>()) }))
}

fn callback(call: impl FnOnce()) -> ::telekio::CallResult {
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(()) => ::telekio::CallResult::ok(),
        Err(payload) => ::telekio::CallResult::panicked(&*payload),
    }
}
