use super::{AbortHandle, Header, Notified, OwnedTasks, Schedule, Task, UnownedTask};
use crate::runtime::task;
use std::{
    collections::HashMap,
    future::Future,
    panic::{catch_unwind, AssertUnwindSafe},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    task::{Context, Poll, Waker},
    time::Duration,
};

pub(crate) trait HostSchedule: Schedule + Clone + Send + Sync + 'static {
    fn registry(&self) -> &Registry<Self>;
    fn run(&self, task: Notified<Self>) -> u64;
    fn enter<R>(&self, call: impl FnOnce() -> R) -> R;
}

pub(crate) struct Host {
    runtime: Option<::telekio::Runtime>,
    handle: ::telekio::Handle,
    #[cfg(tokio_unstable)]
    panicked: AtomicBool,
    #[cfg(tokio_unstable)]
    roots: Mutex<Vec<std::sync::Weak<RootState>>>,
    #[cfg(tokio_unstable)]
    workers: Arc<Workers>,
    #[cfg(tokio_unstable)]
    observing: OnceLock<()>,
}

#[cfg(tokio_unstable)]
struct Workers {
    threads: Mutex<Vec<Option<std::thread::ThreadId>>>,
}

pub(crate) struct Registry<S: HostSchedule> {
    host: OnceLock<Arc<Host>>,
    runners: Mutex<HashMap<task::Id, Arc<Runner<S>>>>,
    #[cfg(feature = "taskdump")]
    tracing: TraceLock,
}

struct Runner<S: HostSchedule> {
    notified: Mutex<Option<Notified<S>>>,
    waker: Mutex<Option<Waker>>,
    abort: AbortHandle,
    complete: AtomicBool,
    #[cfg(feature = "taskdump")]
    task: Task<S>,
    #[cfg(feature = "taskdump")]
    trace: Mutex<Option<Arc<TraceRequest>>>,
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

struct HostCall {
    started: std::time::Instant,
    outer: bool,
}

thread_local! {
    static HOST_CALLS: std::cell::Cell<(usize, u64)> = const { std::cell::Cell::new((0, 0)) };
}

#[cfg(tokio_unstable)]
struct RootState {
    waker: Mutex<Option<Waker>>,
}

#[cfg(tokio_unstable)]
struct Root<'a, F> {
    future: F,
    host: &'a Host,
    state: Arc<RootState>,
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

pub(crate) fn budget<F: Future>(future: F) -> impl Future<Output = F::Output> {
    Budgeted(future)
}

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

impl<F: Future> Future for Budgeted<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let future = unsafe { self.map_unchecked_mut(|budgeted| &mut budgeted.0) };
        crate::task::coop::budget(|| future.poll(context))
    }
}

#[cfg(tokio_unstable)]
impl<F: Future> Future for Root<'_, F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        this.host.record_worker();
        *this.state.waker.lock().unwrap() = Some(context.waker().clone());
        this.host.check_panic();
        let result = unsafe { Pin::new_unchecked(&mut this.future) }.poll(context);
        this.host.check_panic();
        result
    }
}

#[cfg(tokio_unstable)]
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
    pub(crate) fn new(runtime: ::telekio::Runtime) -> Arc<Self> {
        Arc::new(Self {
            handle: runtime.handle(),
            runtime: Some(runtime),
            #[cfg(tokio_unstable)]
            panicked: AtomicBool::new(false),
            #[cfg(tokio_unstable)]
            roots: Mutex::new(Vec::new()),
            #[cfg(tokio_unstable)]
            workers: Arc::new(Workers::new()),
            #[cfg(tokio_unstable)]
            observing: OnceLock::new(),
        })
    }

    #[cfg(not(test))]
    pub(crate) fn attached(handle: ::telekio::Handle) -> Arc<Self> {
        let host = Arc::new(Self {
            runtime: None,
            handle,
            #[cfg(tokio_unstable)]
            panicked: AtomicBool::new(false),
            #[cfg(tokio_unstable)]
            roots: Mutex::new(Vec::new()),
            #[cfg(tokio_unstable)]
            workers: Arc::new(Workers::new()),
            #[cfg(tokio_unstable)]
            observing: OnceLock::new(),
        });
        host
    }

    pub(crate) fn defer(&self, waker: &::telekio::Waker) -> ::telekio::CallResult {
        host_call(|| self.handle.defer(waker))
    }

    pub(crate) fn metric(&self, metric: ::telekio::Metric, worker: usize) -> u64 {
        host_call(|| self.handle.metric(metric, worker))
    }

    #[cfg(tokio_unstable)]
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

    #[cfg(tokio_unstable)]
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

    #[cfg(tokio_unstable)]
    pub(crate) fn record_worker(&self) {
        if let Some(worker) = self.worker_index() {
            self.store_worker(worker);
        }
    }

    #[cfg(tokio_unstable)]
    fn store_worker(&self, worker: usize) {
        self.workers.store(worker);
    }

    #[cfg(tokio_unstable)]
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
    pub(crate) fn unhandled_panic(&self) {
        self.panicked.store(true, Ordering::Release);
        let mut roots = self.roots.lock().unwrap();
        roots.retain(|root| root.strong_count() != 0);
        let wakers = roots
            .iter()
            .filter_map(std::sync::Weak::upgrade)
            .filter_map(|root| root.waker.lock().unwrap().clone())
            .collect::<Vec<_>>();
        drop(roots);
        for waker in wakers {
            waker.wake();
        }
    }

    #[cfg(tokio_unstable)]
    fn check_panic(&self) {
        if self.panicked.load(Ordering::Acquire) {
            panic!(
                "a spawned task panicked and the runtime is configured to shut down on unhandled panic"
            );
        }
    }

    #[cfg(tokio_unstable)]
    fn root<F: Future>(&self, future: F) -> Root<'_, F> {
        let state = Arc::new(RootState {
            waker: Mutex::new(None),
        });
        self.roots.lock().unwrap().push(Arc::downgrade(&state));
        Root {
            future,
            host: self,
            state,
        }
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

    #[cfg(feature = "signal")]
    #[cfg_attr(test, expect(dead_code))]
    pub(crate) fn signal(
        &self,
        request: ::telekio::SignalRequest,
    ) -> std::io::Result<::telekio::Signal> {
        self.handle.signal(request)
    }

    #[cfg(any(
        feature = "net",
        all(unix, any(feature = "process", feature = "signal"))
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
fn trace_notified<S: HostSchedule>(
    schedule: &S,
    notified: Notified<S>,
) -> (u64, (task::Id, super::trace::Trace)) {
    let id = notified.id();
    let waker = super::waker::waker_ref::<S>(notified.0.raw.header_ptr_ref());
    crate::runtime::context::telekio::defer(&waker);
    let (duration, trace) = super::trace::Trace::capture(|| schedule.run(notified));
    (duration, (id, trace))
}

impl<S: HostSchedule> Registry<S> {
    pub(crate) fn new() -> Self {
        Self {
            host: OnceLock::new(),
            runners: Mutex::new(HashMap::new()),
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

    #[cfg(feature = "taskdump")]
    pub(crate) fn dump_current(&self) -> Vec<(task::Id, super::trace::Trace)> {
        let current = crate::runtime::context::current_task_id();
        let runners = self
            .runners
            .lock()
            .unwrap()
            .values()
            .filter(|runner| Some(runner.task.telekio_id()) != current)
            .cloned()
            .collect::<Vec<_>>();
        runners
            .into_iter()
            .filter_map(|runner| {
                let notified = runner
                    .notified
                    .lock()
                    .unwrap()
                    .take()
                    .or_else(|| runner.task.notify_for_tracing())?;
                let schedule = notified.schedule();
                Some(trace_notified(&schedule, notified).1)
            })
            .collect()
    }

    #[cfg(feature = "taskdump")]
    pub(crate) async fn dump(&self) -> Vec<(task::Id, super::trace::Trace)> {
        let _guard = self.tracing.lock().await;
        let runners = self
            .runners
            .lock()
            .unwrap()
            .values()
            .filter(|runner| !runner.complete.load(Ordering::Acquire))
            .cloned()
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
            } else if let Some(notified) = runner.task.notify_for_tracing() {
                runner.schedule(notified);
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

    pub(crate) fn schedule(&self, schedule: S, notified: Notified<S>, local: bool) {
        let _call = HostCall::new();
        let id = notified.id();
        let mut runners = self.runners.lock().unwrap();
        if let Some(runner) = runners.get(&id) {
            runner.schedule(notified);
            return;
        }

        #[cfg(feature = "taskdump")]
        let trace_task = notified.trace_task();
        let runner = Arc::new(Runner {
            abort: notified.abort_handle(),
            notified: Mutex::new(Some(notified)),
            waker: Mutex::new(None),
            complete: AtomicBool::new(false),
            #[cfg(feature = "taskdump")]
            task: trace_task,
            #[cfg(feature = "taskdump")]
            trace: Mutex::new(None),
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
        if let Some(runner) = self.runners.lock().unwrap().remove(&task.telekio_id()) {
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
        host_call(|| self.host().handle.spawn_blocking(task))
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

    #[cfg(tokio_unstable)]
    pub(crate) fn telekio_task_meta<'meta>(&self) -> crate::runtime::TaskMeta<'meta> {
        self.0.task_meta()
    }

    #[cfg(feature = "taskdump")]
    fn trace_task(&self) -> Task<S> {
        let raw = self.0.raw.clone();
        raw.ref_inc();
        Task {
            raw,
            _p: std::marker::PhantomData,
        }
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
    fn telekio_id(&self) -> task::Id {
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

    #[cfg(feature = "taskdump")]
    fn schedule_trace(&self) {
        if self.complete.load(Ordering::Acquire) || self.trace.lock().unwrap().is_none() {
            return;
        }
        if let Some(notified) = self.task.notify_for_tracing() {
            self.schedule(notified);
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
    waker: *const ::telekio::Waker,
) -> ::telekio::TaskPoll {
    match catch_unwind(AssertUnwindSafe(|| unsafe {
        poll_runner_inner::<S>(data, waker)
    })) {
        Ok(poll) => poll,
        Err(_) => {
            let task = unsafe { &mut *data.cast::<RunnerTask<S>>() };
            task.runner.abort.abort();
            ::telekio::TaskPoll::unmeasured(::telekio::Poll::Panicked)
        }
    }
}

unsafe fn poll_runner_inner<S: HostSchedule>(
    data: *mut std::ffi::c_void,
    waker: *const ::telekio::Waker,
) -> ::telekio::TaskPoll {
    let task = unsafe { &mut *data.cast::<RunnerTask<S>>() };
    *task.runner.waker.lock().unwrap() = Some(unsafe { (*waker).clone_rust_waker() });
    if task.runner.complete.load(Ordering::Acquire) {
        return ::telekio::TaskPoll::unmeasured(::telekio::Poll::Ready);
    }
    let mut duration = None;
    let notified = task.runner.notified.lock().unwrap().take();
    if let Some(notified) = notified {
        #[cfg(feature = "taskdump")]
        {
            let request = task.runner.trace.lock().unwrap().take();
            if let Some(request) = request {
                let completion = TraceCompletion(Some(request));
                let (measured, trace) = trace_notified(&task.schedule, notified);
                duration = Some(measured);
                completion.complete(trace);
            } else {
                duration = Some(task.schedule.run(notified));
            }
        }
        #[cfg(not(feature = "taskdump"))]
        {
            duration = Some(task.schedule.run(notified));
        }
    }
    let state = if task.runner.complete.load(Ordering::Acquire) {
        ::telekio::Poll::Ready
    } else {
        #[cfg(feature = "taskdump")]
        task.runner.schedule_trace();
        if task.runner.notified.lock().unwrap().is_some() {
            task.runner.wake();
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
) -> ::telekio::CallResult {
    callback(|| {
        let task = unsafe { &mut *data.cast::<RunnerTask<S>>() };
        task.runner.abort.abort();
        let notified = task.runner.notified.lock().unwrap().take();
        if let Some(notified) = notified {
            _ = task.schedule.run(notified);
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
