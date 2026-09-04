use super::{
    AbortHandle, Notified, OwnedTasks, Schedule, SpawnLocation, Task, TaskHarnessScheduleHooks,
    UnownedTask,
};
use crate::{
    future::Future as TaskFuture,
    runtime::{TaskHooks, TaskMeta, task},
};
#[cfg(any(tokio_unstable, feature = "taskdump"))]
use std::task::Poll;
use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    task::Waker,
    time::Duration,
};
#[cfg(tokio_unstable)]
use std::{future::Future, pin::Pin, task::Context};

pub(crate) trait HostSchedule: Schedule + Clone + Send + Sync + 'static {
    fn registry(&self) -> &Registry<Self>;
    fn run(&self, call: impl FnOnce()) -> u64;
    fn enter<R>(&self, call: impl FnOnce() -> R) -> R;
}

pub(crate) struct Host {
    runtime: Option<::telekio::Runtime>,
    handle: ::telekio::Handle,
    #[cfg_attr(
        not(any(feature = "signal", all(unix, feature = "process"))),
        expect(dead_code)
    )]
    io_enabled: bool,
    task_hooks: Arc<HostTaskHooks>,
    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    workers: Arc<Workers>,
    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    observing: OnceLock<()>,
}

#[cfg(all(tokio_unstable, target_has_atomic = "64"))]
struct Workers {
    threads: Mutex<Vec<Option<std::thread::ThreadId>>>,
}

pub(crate) struct HostTaskHooks {
    hooks: TaskHooks,
    tasks: Mutex<HashMap<task::Id, TaskHookState>>,
    active: bool,
}

struct TaskHookState {
    location: SpawnLocation,
    polling: bool,
    terminated: bool,
}

pub(crate) struct Registry<S: HostSchedule> {
    host: OnceLock<Arc<Host>>,
    marker: std::marker::PhantomData<fn() -> S>,
    #[cfg(feature = "taskdump")]
    runners: Mutex<Vec<Weak<Runner<S>>>>,
    #[cfg(feature = "taskdump")]
    tracing: TraceLock,
}

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
    id: task::Id,
    #[cfg(feature = "taskdump")]
    task: OnceLock<Task<TaskSchedule<S>>>,
    #[cfg(feature = "taskdump")]
    trace: Mutex<Option<Arc<TraceRequest>>>,
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

struct HostCall {
    started: std::time::Instant,
    outer: bool,
}

thread_local! {
    static HOST_CALLS: std::cell::Cell<(usize, u64)> = const { std::cell::Cell::new((0, 0)) };
    static SPAWN_LOCATION: std::cell::Cell<Option<::telekio::SourceLocation>> = const { std::cell::Cell::new(None) };
}

impl SpawnLocation {
    #[track_caller]
    pub(crate) fn capture() -> Self {
        SPAWN_LOCATION.set(Some(::telekio::SourceLocation::caller()));
        Self::capture_local()
    }

    pub(crate) fn take_telekio() -> ::telekio::SourceLocation {
        SPAWN_LOCATION
            .take()
            .expect("Tokio spawn location is missing")
    }
}

impl HostTaskHooks {
    #[cfg(not(test))]
    pub(crate) fn empty() -> Arc<Self> {
        Self::new(TaskHooks {
            task_spawn_callback: None,
            task_terminate_callback: None,
            #[cfg(tokio_unstable)]
            before_poll_callback: None,
            #[cfg(tokio_unstable)]
            after_poll_callback: None,
        })
    }

    pub(crate) fn new(hooks: TaskHooks) -> Arc<Self> {
        let active = Self::hooks_active(&hooks);
        Arc::new(Self {
            hooks,
            tasks: Mutex::new(HashMap::new()),
            active,
        })
    }

    fn hooks_active(hooks: &TaskHooks) -> bool {
        hooks.task_spawn_callback.is_some() || hooks.task_terminate_callback.is_some() || {
            #[cfg(tokio_unstable)]
            {
                hooks.before_poll_callback.is_some() || hooks.after_poll_callback.is_some()
            }
            #[cfg(not(tokio_unstable))]
            {
                false
            }
        }
    }

    pub(crate) fn callback(self: &Arc<Self>) -> ::telekio::TaskCallback {
        if !self.active {
            return ::telekio::TaskCallback::none();
        }
        let hooks = Arc::clone(self);
        ::telekio::TaskCallback::from_arc(Arc::new(move |event, id| hooks.call(event, id)))
    }

    fn register(&self, id: task::Id, location: SpawnLocation) {
        if !self.active {
            return;
        }
        let previous = self.tasks.lock().unwrap().insert(
            id,
            TaskHookState {
                location,
                polling: false,
                terminated: false,
            },
        );
        assert!(previous.is_none(), "Tokio task hook was registered twice");
    }

    fn remove(&self, id: task::Id) {
        if self.active {
            self.tasks.lock().unwrap().remove(&id);
        }
    }

    fn call(&self, event: ::telekio::TaskEvent, id: u64) {
        let id = task::Id::from_telekio(id);
        let mut tasks = self.tasks.lock().unwrap();
        let state = tasks
            .get_mut(&id)
            .expect("Tokio task hook received an unknown task");
        let spawned_at = state.location;
        match event {
            ::telekio::TaskEvent::PollStart => state.polling = true,
            ::telekio::TaskEvent::PollStop => state.polling = false,
            ::telekio::TaskEvent::Terminate => state.terminated = true,
            ::telekio::TaskEvent::Spawn => {}
        }
        if state.terminated && !state.polling {
            tasks.remove(&id);
        }
        drop(tasks);
        let meta = TaskMeta {
            id,
            spawned_at,
            _phantom: Default::default(),
        };
        match event {
            ::telekio::TaskEvent::Spawn => self.hooks.spawn(&meta),
            ::telekio::TaskEvent::PollStart => {
                #[cfg(tokio_unstable)]
                self.hooks.poll_start_callback(&meta);
            }
            ::telekio::TaskEvent::PollStop => {
                #[cfg(tokio_unstable)]
                self.hooks.poll_stop_callback(&meta);
            }
            ::telekio::TaskEvent::Terminate => {
                if let Some(callback) = &self.hooks.task_terminate_callback {
                    callback(&meta);
                }
            }
        }
    }
}

#[cfg(tokio_unstable)]
struct Root<'a, F> {
    future: F,
    host: &'a Host,
}

#[cfg(feature = "taskdump")]
struct TraceRequest {
    remaining: std::sync::atomic::AtomicUsize,
    traces: Mutex<Vec<(task::Id, super::trace::Trace)>>,
    waker: Mutex<Option<Waker>>,
}

#[cfg(feature = "taskdump")]
#[derive(Default)]
struct TraceLock {
    state: Mutex<TraceLockState>,
}

#[cfg(feature = "taskdump")]
#[derive(Default)]
struct TraceLockState {
    active: bool,
    waiters: Vec<Waker>,
}

#[cfg(feature = "taskdump")]
struct TraceGuard<'a>(&'a TraceLock);

#[cfg(feature = "taskdump")]
struct TraceCompletion(Option<Arc<TraceRequest>>);

pub(crate) fn measure_poll(call: impl FnOnce()) -> u64 {
    let host_before = HOST_CALLS.get().1;
    let started = std::time::Instant::now();
    call();
    let elapsed = started.elapsed().as_nanos().min(u64::MAX.into()) as u64;
    elapsed.saturating_sub(HOST_CALLS.get().1.saturating_sub(host_before))
}

fn host_call<T>(call: impl FnOnce() -> T) -> T {
    let _call = HostCall::new();
    call()
}

impl HostCall {
    fn new() -> Self {
        let (depth, elapsed) = HOST_CALLS.get();
        HOST_CALLS.set((depth + 1, elapsed));
        Self {
            started: std::time::Instant::now(),
            outer: depth == 0,
        }
    }
}

impl Drop for HostCall {
    fn drop(&mut self) {
        let (depth, elapsed) = HOST_CALLS.get();
        let elapsed = if self.outer {
            elapsed.saturating_add(self.started.elapsed().as_nanos().min(u64::MAX.into()) as u64)
        } else {
            elapsed
        };
        HOST_CALLS.set((depth - 1, elapsed));
    }
}

#[cfg(tokio_unstable)]
impl<F: Future> Future for Root<'_, F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        #[cfg(target_has_atomic = "64")]
        this.host.record_worker();
        unsafe { Pin::new_unchecked(&mut this.future) }.poll(context)
    }
}

#[cfg(all(tokio_unstable, target_has_atomic = "64"))]
impl Workers {
    fn new() -> Self {
        Self {
            threads: Mutex::new(Vec::new()),
        }
    }

    fn store(&self, worker: usize) {
        let mut threads = self.threads.lock().unwrap();
        if threads.len() <= worker {
            threads.resize(worker + 1, None);
        }
        threads[worker] = Some(std::thread::current().id());
    }

    fn get(&self, worker: usize) -> Option<std::thread::ThreadId> {
        self.threads.lock().unwrap().get(worker).copied().flatten()
    }
}

impl Host {
    pub(crate) fn new(
        runtime: ::telekio::Runtime,
        io_enabled: bool,
        task_hooks: Arc<HostTaskHooks>,
    ) -> Arc<Self> {
        Arc::new(Self {
            handle: runtime.handle(),
            runtime: Some(runtime),
            io_enabled,
            task_hooks,
            #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
            workers: Arc::new(Workers::new()),
            #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
            observing: OnceLock::new(),
        })
    }

    #[cfg(not(test))]
    pub(crate) fn attached(handle: ::telekio::Handle) -> Arc<Self> {
        let host = Arc::new(Self {
            runtime: None,
            handle,
            io_enabled: true,
            task_hooks: HostTaskHooks::empty(),
            #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
            workers: Arc::new(Workers::new()),
            #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
            observing: OnceLock::new(),
        });
        host
    }

    pub(crate) fn unhandled_panic(&self) {
        host_call(|| self.handle.task_panicked()).resume("failed to apply Tokio panic policy");
    }

    pub(crate) fn abort(&self, id: task::Id) {
        host_call(|| self.handle.abort(id.as_u64())).resume("failed to abort Tokio task");
    }

    pub(crate) fn defer(&self, waker: &::telekio::Waker) -> ::telekio::CallResult {
        host_call(|| self.handle.defer(waker))
    }

    pub(crate) fn metric(&self, metric: ::telekio::Metric, worker: usize) -> u64 {
        host_call(|| self.handle.metric(metric, worker))
    }

    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    pub(crate) fn metric_bucket(
        &self,
        metric: ::telekio::Metric,
        worker: usize,
        bucket: usize,
    ) -> u64 {
        host_call(|| self.handle.metric_bucket(metric, worker, bucket))
    }

    #[cfg(tokio_unstable)]
    pub(crate) fn worker_index(&self) -> Option<usize> {
        usize::try_from(
            self.metric(::telekio::Metric::CurrentWorkerIndex, 0)
                .checked_sub(1)?,
        )
        .ok()
    }

    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    fn observe_workers(self: &Arc<Self>) {
        self.observing.get_or_init(|| {
            let workers = Arc::clone(&self.workers);
            let callback = ::telekio::WorkerCallback::from_arc(Arc::new(move |worker| {
                workers.store(worker);
            }));
            self.handle
                .observe_workers(callback)
                .resume("failed to observe Tokio worker threads");
        });
    }

    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    pub(crate) fn record_worker(&self) {
        if let Some(worker) = self.worker_index() {
            self.store_worker(worker);
        }
    }

    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    fn store_worker(&self, worker: usize) {
        self.workers.store(worker);
    }

    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    pub(crate) fn worker_thread_id(
        self: &Arc<Self>,
        worker: usize,
    ) -> Option<std::thread::ThreadId> {
        assert!(worker < self.metric(::telekio::Metric::NumWorkers, 0) as usize);
        self.observe_workers();
        self.workers.get(worker)
    }

    pub(crate) fn flavor(&self) -> ::telekio::Flavor {
        self.handle.flavor()
    }

    pub(crate) fn id(&self) -> u64 {
        self.handle.id()
    }

    pub(crate) fn runtime_block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        #[cfg(tokio_unstable)]
        let future = self.root(future);
        match &self.runtime {
            Some(runtime) => runtime.block_on(future),
            None => self.handle.block_on(future),
        }
    }

    pub(crate) fn handle_block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        #[cfg(tokio_unstable)]
        let future = self.root(future);
        self.handle.block_on(future)
    }

    #[cfg(tokio_unstable)]
    fn root<F: Future>(&self, future: F) -> Root<'_, F> {
        Root { future, host: self }
    }

    #[cfg(feature = "rt-multi-thread")]
    pub(crate) fn block_in_place(&self, blocking: ::telekio::Blocking) -> ::telekio::CallResult {
        self.handle.block_in_place(blocking)
    }

    #[cfg(feature = "test-util")]
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

    #[cfg(feature = "time")]
    pub(crate) fn timer(&self, deadline: std::time::Instant) -> ::telekio::Timer {
        ::telekio::Timer::from_result(self.handle.timer(deadline))
    }

    #[cfg(any(feature = "signal", all(unix, feature = "process")))]
    #[cfg_attr(test, expect(dead_code))]
    #[track_caller]
    pub(crate) fn signal(
        &self,
        request: ::telekio::SignalRequest,
    ) -> std::io::Result<::telekio::Signal> {
        assert!(
            self.io_enabled,
            "there is no signal driver running, must be called from the context of Tokio runtime"
        );
        self.handle.signal(request)
    }

    #[cfg(all(
        tokio_unstable,
        feature = "io-uring",
        feature = "rt",
        feature = "fs",
        target_os = "linux"
    ))]
    pub(crate) fn register_io_driver(
        &self,
        resource: ::telekio::IoResource,
        callback: ::telekio::Callback,
    ) -> ::telekio::IoDriverResult {
        self.handle.register_io_driver(resource, callback)
    }

    #[cfg(any(
        feature = "net",
        all(unix, any(feature = "process", feature = "signal")),
        all(unix, feature = "rt", feature = "fs", feature = "io-uring")
    ))]
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
        if let Some(runtime) = &self.runtime {
            runtime
                .shutdown(mode, duration.as_secs(), duration.subsec_nanos())
                .into_io_result()
                .expect("failed to shut down the Tokio runtime");
        }
    }
}

#[cfg(feature = "taskdump")]
impl TraceLock {
    async fn lock(&self) -> TraceGuard<'_> {
        std::future::poll_fn(|context| {
            let mut state = self.state.lock().unwrap();
            if !state.active {
                state.active = true;
                Poll::Ready(TraceGuard(self))
            } else {
                if !state
                    .waiters
                    .iter()
                    .any(|waker| waker.will_wake(context.waker()))
                {
                    state.waiters.push(context.waker().clone());
                }
                Poll::Pending
            }
        })
        .await
    }
}

#[cfg(feature = "taskdump")]
impl Drop for TraceGuard<'_> {
    fn drop(&mut self) {
        let waiters = {
            let mut state = self.0.state.lock().unwrap();
            state.active = false;
            std::mem::take(&mut state.waiters)
        };
        for waker in waiters {
            waker.wake();
        }
    }
}

#[cfg(feature = "taskdump")]
impl TraceCompletion {
    fn complete(mut self, trace: (task::Id, super::trace::Trace)) {
        self.0.take().unwrap().complete(Some(trace));
    }
}

#[cfg(feature = "taskdump")]
impl Drop for TraceCompletion {
    fn drop(&mut self) {
        if let Some(request) = self.0.take() {
            request.complete(None);
        }
    }
}

#[cfg(feature = "taskdump")]
impl TraceRequest {
    fn complete(&self, trace: Option<(task::Id, super::trace::Trace)>) {
        if let Some(trace) = trace {
            self.traces.lock().unwrap().push(trace);
        }
        if self.remaining.fetch_sub(1, Ordering::AcqRel) == 1 {
            if let Some(waker) = self.waker.lock().unwrap().take() {
                waker.wake();
            }
        }
    }
}

#[cfg(feature = "taskdump")]
fn trace_runnable<S: HostSchedule>(
    runner: &Runner<S>,
    runnable: Runnable<S>,
) -> (u64, (task::Id, super::trace::Trace)) {
    let task = runner.task.get().unwrap();
    let waker = super::waker::waker_ref::<TaskSchedule<S>>(task.raw.header_ptr_ref());
    crate::runtime::context::telekio::defer(&waker);
    let (duration, trace) = super::trace::Trace::capture(|| runner.run(runnable));
    (duration, (runner.id, trace))
}

impl<S: HostSchedule> Registry<S> {
    pub(crate) fn new() -> Self {
        Self {
            host: OnceLock::new(),
            marker: std::marker::PhantomData,
            #[cfg(feature = "taskdump")]
            runners: Mutex::new(Vec::new()),
            #[cfg(feature = "taskdump")]
            tracing: TraceLock::default(),
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

    fn start(&self, runner: Arc<Runner<S>>, local: bool, location: ::telekio::SourceLocation) {
        #[cfg(feature = "taskdump")]
        self.runners.lock().unwrap().push(Arc::downgrade(&runner));
        let id = runner.id;
        let task = unsafe {
            ::telekio::Task::from_raw(
                Arc::into_raw(runner).cast_mut().cast(),
                poll_runner::<S>,
                cancel_runner::<S>,
                release_runner::<S>,
            )
        };
        let result = if local {
            self.host().handle.spawn_local(task, id.as_u64(), location)
        } else {
            self.host().handle.spawn(task, id.as_u64(), location)
        };
        if let Err(error) = result.into_io_result() {
            self.host().task_hooks.remove(id);
            panic!("failed to spawn Tokio task {id}: {error}");
        }
    }

    fn bind<T>(
        &self,
        schedule: S,
        future: T,
        id: task::Id,
        spawned_at: SpawnLocation,
        local: bool,
        location: ::telekio::SourceLocation,
    ) -> task::JoinHandle<T::Output>
    where
        T: TaskFuture + Send + 'static,
        T::Output: Send + 'static,
    {
        self.host().task_hooks.register(id, spawned_at);
        let runner = Runner::new(schedule.clone(), id);
        let task_schedule = TaskSchedule {
            runner: Arc::downgrade(&runner),
        };
        let (task, join) = task::unowned(future, task_schedule, id, spawned_at);
        runner.initialize(task, &join);
        self.start(runner, local, location);
        join
    }

    unsafe fn bind_local<T>(
        &self,
        schedule: S,
        future: T,
        id: task::Id,
        spawned_at: SpawnLocation,
        location: ::telekio::SourceLocation,
    ) -> task::JoinHandle<T::Output>
    where
        T: TaskFuture + 'static,
        T::Output: 'static,
    {
        self.host().task_hooks.register(id, spawned_at);
        let runner = Runner::new(schedule.clone(), id);
        let task_schedule = TaskSchedule {
            runner: Arc::downgrade(&runner),
        };
        let (task, join) = unsafe { unowned_local(future, task_schedule, id, spawned_at) };
        runner.initialize(task, &join);
        self.start(runner, true, location);
        join
    }

    #[cfg(feature = "taskdump")]
    fn runners(&self) -> Vec<Arc<Runner<S>>> {
        let mut runners = self.runners.lock().unwrap();
        let active = runners.iter().filter_map(Weak::upgrade).collect::<Vec<_>>();
        runners.retain(|runner| runner.strong_count() != 0);
        active
    }

    #[cfg(feature = "taskdump")]
    pub(crate) fn dump_current(&self) -> Vec<(task::Id, super::trace::Trace)> {
        let current = crate::runtime::context::current_task_id();
        self.runners()
            .into_iter()
            .filter(|runner| Some(runner.id) != current)
            .filter_map(|runner| {
                let runnable = runner.take_or_notify()?;
                Some(trace_runnable(&runner, runnable).1)
            })
            .collect()
    }

    #[cfg(feature = "taskdump")]
    pub(crate) async fn dump(&self) -> Vec<(task::Id, super::trace::Trace)> {
        let _guard = self.tracing.lock().await;
        let runners = self
            .runners()
            .into_iter()
            .filter(|runner| !runner.complete.load(Ordering::Acquire))
            .collect::<Vec<_>>();
        let request = Arc::new(TraceRequest {
            remaining: std::sync::atomic::AtomicUsize::new(runners.len()),
            traces: Mutex::new(Vec::new()),
            waker: Mutex::new(None),
        });
        for runner in &runners {
            *runner.trace.lock().unwrap() = Some(Arc::clone(&request));
            if runner.complete.load(Ordering::Acquire) {
                if let Some(request) = runner.trace.lock().unwrap().take() {
                    request.complete(None);
                }
            } else if let Some(runnable) = runner.notify_for_tracing() {
                runner.schedule(runnable);
            } else {
                runner.wake();
            }
        }
        std::future::poll_fn(|context| {
            if request.remaining.load(Ordering::Acquire) == 0 {
                return Poll::Ready(());
            }
            *request.waker.lock().unwrap() = Some(context.waker().clone());
            if request.remaining.load(Ordering::Acquire) == 0 {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        let traces = std::mem::take(&mut *request.traces.lock().unwrap());
        traces
    }

    pub(crate) fn spawn_blocking<F, R>(
        &self,
        schedule: S,
        function: F,
        id: task::Id,
        spawned_at: task::SpawnLocation,
        location: ::telekio::SourceLocation,
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
            schedule,
            task: Mutex::new(None),
            id,
        });
        let task_schedule = BlockingSchedule {
            runner: Arc::downgrade(&runner),
        };
        let (task, join) = task::unowned(future, task_schedule, id, spawned_at);
        *runner.task.lock().unwrap() = Some(task);
        self.host().task_hooks.register(id, spawned_at);
        let task = unsafe {
            ::telekio::BlockingTask::from_raw(
                Arc::into_raw(runner).cast_mut().cast(),
                run_blocking::<S>,
                cancel_blocking::<S>,
                release_blocking::<S>,
            )
        };
        if let Err(error) = host_call(|| {
            self.host()
                .handle
                .spawn_blocking(task, id.as_u64(), location)
        })
        .into_io_result()
        {
            self.host().task_hooks.remove(id);
            panic!("failed to spawn blocking Tokio task {id}: {error}");
        }
        join
    }
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

impl crate::runtime::TaskHooks {
    pub(crate) fn spawn_host(&self, _: &TaskMeta<'_>) {}
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
        let join = schedule.registry().bind(
            schedule.clone(),
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
            schedule.registry().bind_local(
                schedule.clone(),
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
                runner.schedule.registry().host().abort(runner.id);
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
            runner.schedule.registry().host().unhandled_panic();
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
                runner.schedule.registry().host().abort(runner.id);
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
            runner.schedule.registry().host().unhandled_panic();
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
    let (task, notified, join) = super::new_task(future, schedule, id, spawned_at);
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
            id,
            #[cfg(feature = "taskdump")]
            task: OnceLock::new(),
            #[cfg(feature = "taskdump")]
            trace: Mutex::new(None),
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
        self.runnable.lock().unwrap().take().or_else(|| {
            self.task
                .get()
                .and_then(Task::notify_for_tracing)
                .map(Runnable::Notified)
        })
    }

    #[cfg(feature = "taskdump")]
    fn notify_for_tracing(&self) -> Option<Runnable<S>> {
        self.task
            .get()
            .and_then(Task::notify_for_tracing)
            .map(Runnable::Notified)
    }

    #[cfg(feature = "taskdump")]
    fn schedule_trace(&self) {
        if self.complete.load(Ordering::Acquire) || self.trace.lock().unwrap().is_none() {
            return;
        }
        if let Some(runnable) = self.notify_for_tracing() {
            self.schedule(runnable);
        } else {
            self.wake();
        }
    }

    fn finish(&self) {
        self.complete.store(true, Ordering::Release);
        #[cfg(feature = "taskdump")]
        if let Some(request) = self.trace.lock().unwrap().take() {
            request.complete(None);
        }
        self.wake();
    }
}

unsafe extern "C" fn poll_runner<S: HostSchedule>(
    data: *mut std::ffi::c_void,
    execution: *mut ::telekio::ExecutionState,
    waker: *const ::telekio::Waker,
) -> ::telekio::TaskPoll {
    match catch_unwind(AssertUnwindSafe(|| unsafe {
        ::telekio::with_execution_state(execution, || poll_runner_inner::<S>(data, waker))
    })) {
        Ok(poll) => poll,
        Err(_) => {
            let runner = unsafe { &*data.cast::<Runner<S>>() };
            runner.abort.get().unwrap().abort();
            ::telekio::TaskPoll::unmeasured(::telekio::Poll::Panicked)
        }
    }
}

unsafe fn poll_runner_inner<S: HostSchedule>(
    data: *mut std::ffi::c_void,
    waker: *const ::telekio::Waker,
) -> ::telekio::TaskPoll {
    let runner = unsafe { &*data.cast::<Runner<S>>() };
    *runner.waker.lock().unwrap() = Some(unsafe { (*waker).clone_rust_waker() });
    if runner.complete.load(Ordering::Acquire) {
        return ::telekio::TaskPoll::unmeasured(::telekio::Poll::Ready);
    }
    let mut duration = None;
    let runnable = runner.runnable.lock().unwrap().take();
    if let Some(runnable) = runnable {
        #[cfg(feature = "taskdump")]
        {
            let request = runner.trace.lock().unwrap().take();
            if let Some(request) = request {
                let completion = TraceCompletion(Some(request));
                let (measured, trace) = trace_runnable(runner, runnable);
                duration = Some(measured);
                completion.complete(trace);
            } else {
                duration = Some(runner.run(runnable));
            }
        }
        #[cfg(not(feature = "taskdump"))]
        {
            duration = Some(runner.run(runnable));
        }
    }
    let state = if runner.complete.load(Ordering::Acquire) {
        ::telekio::Poll::Ready
    } else {
        #[cfg(feature = "taskdump")]
        runner.schedule_trace();
        if runner.runnable.lock().unwrap().is_some() {
            runner.wake();
        }
        ::telekio::Poll::Pending
    };
    duration.map_or_else(
        || ::telekio::TaskPoll::unmeasured(state),
        |duration| ::telekio::TaskPoll::new(state, duration),
    )
}

unsafe extern "C" fn cancel_runner<S: HostSchedule>(
    data: *mut std::ffi::c_void,
    execution: *mut ::telekio::ExecutionState,
) -> ::telekio::CallResult {
    callback(|| unsafe {
        ::telekio::with_execution_state(execution, || {
            let runner = &*data.cast::<Runner<S>>();
            runner.cancel();
        });
    })
}

unsafe extern "C" fn release_runner<S: HostSchedule>(
    data: *mut std::ffi::c_void,
) -> ::telekio::CallResult {
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
    execution: *mut ::telekio::ExecutionState,
) -> ::telekio::CallResult {
    callback(|| unsafe {
        ::telekio::with_execution_state(execution, || {
            (&*data.cast::<BlockingRunner<S>>()).run();
        });
    })
}

unsafe extern "C" fn cancel_blocking<S: HostSchedule>(
    data: *mut std::ffi::c_void,
    execution: *mut ::telekio::ExecutionState,
) -> ::telekio::CallResult {
    callback(|| unsafe {
        ::telekio::with_execution_state(execution, || {
            (&*data.cast::<BlockingRunner<S>>()).cancel();
        });
    })
}

unsafe extern "C" fn release_blocking<S: HostSchedule>(
    data: *mut std::ffi::c_void,
) -> ::telekio::CallResult {
    callback(|| drop(unsafe { Arc::from_raw(data.cast::<BlockingRunner<S>>()) }))
}

fn callback(call: impl FnOnce()) -> ::telekio::CallResult {
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(()) => ::telekio::CallResult::ok(),
        Err(payload) => ::telekio::CallResult::panicked(&*payload),
    }
}
