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
    fn run(&self, task: Notified<Self>);
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
        self.handle.defer(waker)
    }

    pub(crate) fn metric(&self, metric: ::telekio::Metric, worker: usize) -> u64 {
        self.handle.metric(metric, worker)
    }

    #[cfg(tokio_unstable)]
    pub(crate) fn metric_bucket(
        &self,
        metric: ::telekio::Metric,
        worker: usize,
        bucket: usize,
    ) -> u64 {
        self.handle.metric_bucket(metric, worker, bucket)
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
