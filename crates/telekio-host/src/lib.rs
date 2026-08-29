#[path = "io.rs"]
mod host_io;
#[path = "signal.rs"]
mod host_signal;

use std::{
    any::Any,
    cell::{Cell, UnsafeCell},
    collections::HashMap,
    ffi::c_void,
    future::Future as RustFuture,
    io,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    pin::Pin,
    sync::{Arc, Mutex, OnceLock, RwLock, Weak},
    task::{Context as TaskContext, Poll as RustPoll},
    thread::ThreadId,
    time::Duration,
};

use telekio::{
    Blocking, BlockingTask, BuildResult, CallResult, Callback, ClockSample, DurationParts, Flavor,
    Future, InstantOffset, Metric, MetricResult, OperationPoll, OwnedBytes, Poll, RawHandle,
    RawRuntime, RuntimeApi, RuntimeConfig, Shutdown, Status, StringCallback, Task, Timer,
    TimerResult, Waker,
};

pub struct Runtime {
    runtime: Arc<tokio::runtime::Runtime>,
}

pub struct Owner {
    runtime: Arc<tokio::runtime::Runtime>,
    handle: Arc<HandleContext>,
}

struct RuntimeOwner {
    id: OnceLock<u64>,
    owner: Weak<OwnerState>,
    kind: RwLock<RuntimeKind>,
}

enum RuntimeKind {
    Runtime(Option<tokio::runtime::Runtime>),
    Local(Arc<LocalSlot>),
    Closed,
}

struct HandleContext {
    handle: tokio::runtime::Handle,
    owner: Arc<OwnerState>,
    local: Option<Arc<LocalSlot>>,
    io_enabled: bool,
}

struct LocalSlot {
    thread: ThreadId,
    runtime: UnsafeCell<Option<tokio::runtime::LocalRuntime>>,
}

struct OwnerState {
    state: Mutex<OwnerStatus>,
    notify: tokio::sync::Notify,
    shutdown: tokio::sync::Mutex<()>,
}

struct OwnerStatus {
    accepting: bool,
    tasks: HashMap<u64, Option<tokio::task::AbortHandle>>,
    activities: HashMap<u64, Option<std::task::Waker>>,
    runtimes: HashMap<u64, Arc<RuntimeOwner>>,
    resources: HashMap<u64, Arc<dyn OwnerResource>>,
    next_id: u64,
}

trait OwnerResource: Send + Sync {
    fn close(&self);
}

struct HostResource<T: Send> {
    owner: Weak<OwnerState>,
    id: OnceLock<u64>,
    value: Mutex<Option<T>>,
    waker: Mutex<Option<std::task::Waker>>,
}

struct Activity {
    owner: Arc<OwnerState>,
    id: u64,
    _context: OwnerContext,
}

struct OwnerContext {
    previous: *const OwnerState,
}

struct TaskCleanup {
    owner: Arc<OwnerState>,
    id: u64,
}

struct CallbackCleanup {
    owner: Arc<OwnerState>,
    id: u64,
}

struct TimeTimer {
    handle: tokio::runtime::Handle,
    sleep: Pin<Box<tokio::time::Sleep>>,
}

struct CallbackOwner(Callback);
struct StringCallbackOwner(StringCallback);

unsafe impl Send for CallbackOwner {}
unsafe impl Sync for CallbackOwner {}
unsafe impl Send for StringCallbackOwner {}
unsafe impl Sync for StringCallbackOwner {}

// LocalRuntime stays in its originating thread. HandleContext only shares the slot's
// address and checks that thread before every access to the contained runtime.
unsafe impl Send for LocalSlot {}
unsafe impl Sync for LocalSlot {}

thread_local! {
    static ACTIVE_OWNER: Cell<*const OwnerState> = const { Cell::new(std::ptr::null()) };
}

static CLOCK_ORIGIN: OnceLock<std::time::Instant> = OnceLock::new();

static RUNTIME_API: RuntimeApi = RuntimeApi {
    runtime_block_on,
    handle_block_on,
    retain_handle,
    release_handle,
    release_runtime,
    spawn,
    spawn_local,
    spawn_blocking,
    block_in_place,
    abort,
    is_finished,
    build,
    clock,
    pause,
    resume,
    advance,
    timer,
    register_io: host_io::register,
    signal: host_signal::signal,
    reap_process,
    shutdown,
    defer,
    metric,
};

impl Runtime {
    pub fn new() -> io::Result<Self> {
        tokio::runtime::Runtime::new().map(Self::from_tokio)
    }

    pub fn from_tokio(runtime: tokio::runtime::Runtime) -> Self {
        Self {
            runtime: Arc::new(runtime),
        }
    }

    pub fn owner(&self) -> Owner {
        Owner {
            runtime: Arc::clone(&self.runtime),
            handle: handle_context(self.runtime.handle().clone(), owner_state(), None, true),
        }
    }

    pub fn tokio(&self) -> &tokio::runtime::Runtime {
        &self.runtime
    }
}

impl Owner {
    pub fn runtime(&self) -> telekio::Handle {
        unsafe { telekio::Handle::from_abi(raw_handle(Arc::clone(&self.handle))) }
    }

    pub async fn shutdown(&self) -> io::Result<()> {
        if ACTIVE_OWNER.get() == Arc::as_ptr(&self.handle.owner) {
            return Err(io::Error::other(
                "owner shutdown cannot run inside one of its guest callbacks",
            ));
        }
        let _shutdown = self.handle.owner.shutdown.lock().await;
        shutdown_owner(&self.handle.owner, self.runtime.handle()).await
    }
}

fn owner_state() -> Arc<OwnerState> {
    Arc::new(OwnerState {
        state: Mutex::new(OwnerStatus {
            accepting: true,
            tasks: HashMap::new(),
            activities: HashMap::new(),
            runtimes: HashMap::new(),
            resources: HashMap::new(),
            next_id: 1,
        }),
        notify: tokio::sync::Notify::new(),
        shutdown: tokio::sync::Mutex::new(()),
    })
}

impl OwnerState {
    fn is_accepting(&self) -> bool {
        self.state.lock().unwrap().accepting
    }

    fn accepting(&self) -> Result<(), String> {
        self.is_accepting()
            .then_some(())
            .ok_or_else(|| "Tokio owner is shutting down".to_owned())
    }

    fn callback(self: &Arc<Self>, waker: std::task::Waker) -> Option<CallbackCleanup> {
        let mut state = self.state.lock().unwrap();
        if !state.accepting {
            return None;
        }
        let id = state.next_id;
        state.next_id += 1;
        state.activities.insert(id, Some(waker));
        Some(CallbackCleanup {
            owner: Arc::clone(self),
            id,
        })
    }

    fn activity(self: &Arc<Self>) -> Result<Activity, String> {
        let mut state = self.state.lock().unwrap();
        if !state.accepting {
            return Err("Tokio owner is shutting down".to_owned());
        }
        let id = state.next_id;
        state.next_id += 1;
        state.activities.insert(id, None);
        Ok(Activity {
            owner: Arc::clone(self),
            id,
            _context: OwnerContext::enter(self),
        })
    }

    fn update_activity_waker(&self, id: u64, waker: &std::task::Waker) -> bool {
        let mut state = self.state.lock().unwrap();
        if !state.accepting {
            return false;
        }
        if let Some(slot) = state.activities.get_mut(&id) {
            *slot = Some(waker.clone());
        }
        true
    }

    fn wake_activities(&self) {
        let wakers = self
            .state
            .lock()
            .unwrap()
            .activities
            .values()
            .filter_map(Clone::clone)
            .collect::<Vec<_>>();
        for waker in wakers {
            waker.wake();
        }
    }

    fn reserve_task(&self, id: u64) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if !state.accepting {
            return Err("Tokio owner is shutting down".to_owned());
        }
        if state.tasks.contains_key(&id) {
            return Err(format!("task {id} already exists"));
        }
        state.tasks.insert(id, None);
        Ok(())
    }

    fn register_task(&self, id: u64, handle: tokio::task::AbortHandle) {
        let mut state = self.state.lock().unwrap();
        if !state.accepting {
            handle.abort();
        }
        if let Some(slot) = state.tasks.get_mut(&id) {
            *slot = Some(handle);
        }
    }

    fn finish_task(&self, id: u64) {
        self.state.lock().unwrap().tasks.remove(&id);
        self.notify.notify_waiters();
    }

    fn register_runtime(&self, runtime: Arc<RuntimeOwner>) -> Result<u64, String> {
        let mut state = self.state.lock().unwrap();
        if !state.accepting {
            return Err("Tokio owner is shutting down".to_owned());
        }
        let id = state.next_id;
        state.next_id += 1;
        state.runtimes.insert(id, runtime);
        Ok(id)
    }

    fn unregister_runtime(&self, id: u64) {
        self.state.lock().unwrap().runtimes.remove(&id);
    }

    fn register_resource(&self, resource: Arc<dyn OwnerResource>) -> Result<u64, String> {
        let mut state = self.state.lock().unwrap();
        if !state.accepting {
            return Err("Tokio owner is shutting down".to_owned());
        }
        let id = state.next_id;
        state.next_id += 1;
        state.resources.insert(id, resource);
        Ok(id)
    }

    fn unregister_resource(&self, id: u64) {
        self.state.lock().unwrap().resources.remove(&id);
    }

    fn begin_shutdown(
        &self,
    ) -> (
        Vec<tokio::task::AbortHandle>,
        Vec<Arc<dyn OwnerResource>>,
        Vec<std::task::Waker>,
    ) {
        let mut state = self.state.lock().unwrap();
        state.accepting = false;
        let tasks = state.tasks.values().filter_map(Clone::clone).collect();
        let resources = state
            .resources
            .drain()
            .map(|(_, resource)| resource)
            .collect();
        let activities = state.activities.values().filter_map(Clone::clone).collect();
        (tasks, resources, activities)
    }

    async fn wait_idle(&self) {
        loop {
            let notified = self.notify.notified();
            let idle = {
                let state = self.state.lock().unwrap();
                state.tasks.is_empty() && state.activities.is_empty()
            };
            if idle {
                return;
            }
            notified.await;
        }
    }

    fn take_runtimes(&self) -> io::Result<Vec<Arc<RuntimeOwner>>> {
        let mut state = self.state.lock().unwrap();
        if state.runtimes.values().any(|runtime| runtime.local_open()) {
            return Err(io::Error::other(
                "LocalRuntime must be dropped on its originating thread before owner shutdown",
            ));
        }
        Ok(state.runtimes.drain().map(|(_, runtime)| runtime).collect())
    }
}

impl OwnerContext {
    fn enter(owner: &Arc<OwnerState>) -> Self {
        Self {
            previous: ACTIVE_OWNER.replace(Arc::as_ptr(owner)),
        }
    }
}

impl Drop for OwnerContext {
    fn drop(&mut self) {
        ACTIVE_OWNER.set(self.previous);
    }
}

impl Drop for Activity {
    fn drop(&mut self) {
        self.owner.state.lock().unwrap().activities.remove(&self.id);
        self.owner.notify.notify_waiters();
    }
}

impl Drop for TaskCleanup {
    fn drop(&mut self) {
        self.owner.finish_task(self.id);
    }
}

impl Drop for CallbackCleanup {
    fn drop(&mut self) {
        self.owner.state.lock().unwrap().activities.remove(&self.id);
        self.owner.notify.notify_waiters();
    }
}

impl<T: Send + 'static> OwnerResource for HostResource<T> {
    fn close(&self) {
        self.value.lock().unwrap().take();
        if let Some(waker) = self.waker.lock().unwrap().take() {
            waker.wake();
        }
    }
}

impl<T: Send + 'static> HostResource<T> {
    fn new(owner: &Arc<OwnerState>, value: T) -> Result<Arc<Self>, String> {
        let resource = Arc::new(Self {
            owner: Arc::downgrade(owner),
            id: OnceLock::new(),
            value: Mutex::new(Some(value)),
            waker: Mutex::new(None),
        });
        let id = owner.register_resource(Arc::clone(&resource) as Arc<dyn OwnerResource>)?;
        resource.id.set(id).unwrap();
        Ok(resource)
    }

    fn update_waker(&self, waker: &Waker) {
        let value = self.value.lock().unwrap();
        if value.is_some() {
            *self.waker.lock().unwrap() = Some(unsafe { waker.clone_rust_waker() });
        }
    }

    fn with<R>(&self, call: impl FnOnce(&T) -> R) -> Result<R, String> {
        self.value
            .lock()
            .unwrap()
            .as_ref()
            .map(call)
            .ok_or_else(|| "Tokio owner has shut down".to_owned())
    }

    fn with_mut<R>(&self, call: impl FnOnce(&mut T) -> R) -> Result<R, String> {
        self.value
            .lock()
            .unwrap()
            .as_mut()
            .map(call)
            .ok_or_else(|| "Tokio owner has shut down".to_owned())
    }

    fn release(self: Arc<Self>) {
        if let Some(owner) = self.owner.upgrade() {
            owner.unregister_resource(*self.id.get().unwrap());
        }
    }
}

async fn shutdown_owner(
    owner: &Arc<OwnerState>,
    handle: &tokio::runtime::Handle,
) -> io::Result<()> {
    let (tasks, resources, activities) = owner.begin_shutdown();
    for task in tasks {
        task.abort();
    }
    for resource in resources {
        resource.close();
    }
    for activity in activities {
        activity.wake();
    }

    let runtimes = owner.take_runtimes()?;
    let close = handle.spawn_blocking(move || {
        for runtime in runtimes {
            runtime.close(Shutdown::Wait, Duration::ZERO)?;
        }
        Ok::<_, io::Error>(())
    });
    close.await.map_err(io::Error::other)??;
    owner.wait_idle().await;
    Ok(())
}

impl RuntimeOwner {
    fn local_open(&self) -> bool {
        matches!(&*self.kind.read().unwrap(), RuntimeKind::Local(_))
    }

    fn close(&self, mode: Shutdown, duration: Duration) -> io::Result<()> {
        let mut kind = self.kind.write().unwrap();
        match &mut *kind {
            RuntimeKind::Runtime(runtime) => {
                let runtime = runtime.take();
                *kind = RuntimeKind::Closed;
                drop(kind);
                if let Some(runtime) = runtime {
                    shutdown_runtime(runtime, mode, duration);
                }
            }
            RuntimeKind::Local(local) => {
                let runtime = local.take().map_err(io::Error::other)?;
                *kind = RuntimeKind::Closed;
                drop(kind);
                if let Some(runtime) = runtime {
                    shutdown_local(runtime, mode, duration);
                }
            }
            RuntimeKind::Closed => {}
        }
        Ok(())
    }
}

impl LocalSlot {
    fn new(runtime: tokio::runtime::LocalRuntime) -> Self {
        Self {
            thread: std::thread::current().id(),
            runtime: UnsafeCell::new(Some(runtime)),
        }
    }

    fn with<R>(&self, call: impl FnOnce(&tokio::runtime::LocalRuntime) -> R) -> Result<R, String> {
        if std::thread::current().id() != self.thread {
            return Err("LocalRuntime was used from another thread".to_owned());
        }
        let runtime = unsafe { &*self.runtime.get() }
            .as_ref()
            .ok_or_else(|| "Tokio runtime has shut down".to_owned())?;
        Ok(call(runtime))
    }

    fn take(&self) -> Result<Option<tokio::runtime::LocalRuntime>, String> {
        if std::thread::current().id() != self.thread {
            return Err("LocalRuntime was used from another thread".to_owned());
        }
        Ok(unsafe { &mut *self.runtime.get() }.take())
    }
}

impl CallbackOwner {
    fn is_some(&self) -> bool {
        self.0.is_some()
    }

    fn call(&self) {
        let result = self.0.call();
        match result.status {
            Status::Ok => unsafe { result.payload.release() },
            Status::Panicked | Status::HostPanicked | Status::Error => {
                resume_unwind(Box::new(unsafe { result.payload.into_string() }));
            }
        }
    }
}

impl StringCallbackOwner {
    fn call(&self) -> String {
        let result = self.0.call();
        match result.status {
            Status::Ok => unsafe { result.payload.into_string() },
            Status::Panicked | Status::HostPanicked | Status::Error => {
                resume_unwind(Box::new(unsafe { result.payload.into_string() }));
            }
        }
    }
}

unsafe extern "C" fn retain_handle(context: *const c_void) {
    unsafe { Arc::increment_strong_count(context.cast::<HandleContext>()) };
}

unsafe extern "C" fn release_handle(context: *const c_void) {
    unsafe { Arc::decrement_strong_count(context.cast::<HandleContext>()) };
}

unsafe extern "C" fn release_runtime(owner: *mut c_void) {
    let runtime = unsafe { Arc::from_raw(owner.cast::<RuntimeOwner>()) };
    if let Some(owner) = runtime.owner.upgrade() {
        owner.unregister_runtime(*runtime.id.get().unwrap());
    }
}

unsafe extern "C" fn runtime_block_on(owner: *mut c_void, future: Future) -> CallResult {
    let runtime = unsafe { &*owner.cast::<RuntimeOwner>() };
    let owner = runtime.owner.upgrade().expect("Tokio owner has gone away");
    let activity = match owner.activity() {
        Ok(activity) => activity,
        Err(error) => return result(Status::Error, OwnedBytes::from_string(error)),
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let kind = runtime.kind.read().unwrap();
        match &*kind {
            RuntimeKind::Runtime(runtime) => runtime
                .as_ref()
                .expect("Tokio runtime has shut down")
                .block_on(GuestFuture::new(future, &activity)),
            RuntimeKind::Local(runtime) => runtime
                .with(|runtime| runtime.block_on(GuestFuture::new(future, &activity)))
                .unwrap_or_else(|error| panic!("{error}")),
            RuntimeKind::Closed => panic!("Tokio runtime has shut down"),
        }
    }));
    drop(activity);
    block_on_result(outcome)
}

unsafe extern "C" fn handle_block_on(context: *const c_void, future: Future) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    let activity = match context.owner.activity() {
        Ok(activity) => activity,
        Err(error) => return result(Status::Error, OwnedBytes::from_string(error)),
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        context.handle.block_on(GuestFuture::new(future, &activity))
    }));
    drop(activity);
    block_on_result(outcome)
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

unsafe extern "C" fn spawn(context: *const c_void, id: u64, task: Task) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| {
        let task = GuestTask::new(task);
        context.owner.reserve_task(id)?;
        let task = TrackedTask {
            task,
            cleanup: TaskCleanup {
                owner: Arc::clone(&context.owner),
                id,
            },
        };
        let handle = context.handle.spawn(task);
        context.owner.register_task(id, handle.abort_handle());
        drop(handle);
        Ok::<_, String>(())
    })) {
        Ok(Ok(())) => result(Status::Ok, OwnedBytes::empty()),
        Ok(Err(error)) => result(Status::Error, OwnedBytes::from_string(error)),
        Err(payload) => host_panic(&*payload),
    }
}

unsafe extern "C" fn spawn_local(context: *const c_void, id: u64, task: Task) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    let spawned = catch_unwind(AssertUnwindSafe(|| {
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
        let handle = local.with(|runtime| runtime.spawn_local(task))?;
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

unsafe extern "C" fn spawn_blocking(
    context: *const c_void,
    id: u64,
    task: BlockingTask,
) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    let spawned = catch_unwind(AssertUnwindSafe(|| {
        let task = GuestBlockingTask::new(task);
        context.owner.reserve_task(id)?;
        let task = TrackedBlockingTask {
            task,
            cleanup: TaskCleanup {
                owner: Arc::clone(&context.owner),
                id,
            },
        };
        let handle = context.handle.spawn_blocking(move || task.run());
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

unsafe extern "C" fn defer(context: *const c_void, waker: *const Waker) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| {
        let waker = unsafe { (*waker).clone_rust_waker() };
        let Some(cleanup) = context.owner.callback(waker.clone()) else {
            waker.wake();
            return;
        };
        let wake = waker.clone();
        let task = context.handle.spawn(async move {
            tokio::task::yield_now().await;
            waker.wake();
            drop(cleanup);
        });
        if task.is_finished() {
            wake.wake();
        }
    })) {
        Ok(()) => result(Status::Ok, OwnedBytes::empty()),
        Err(payload) => host_panic(&*payload),
    }
}

unsafe extern "C" fn metric(context: *const c_void, metric: Metric, worker: usize) -> MetricResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    if let Err(error) = context.owner.accepting() {
        return MetricResult {
            call: result(Status::Error, OwnedBytes::from_string(error)),
            value: 0,
        };
    }
    match catch_unwind(AssertUnwindSafe(|| {
        let metrics = context.handle.metrics();
        #[cfg(not(target_has_atomic = "64"))]
        let _ = worker;
        match metric {
            Metric::GlobalQueueDepth => metrics.global_queue_depth() as u64,
            Metric::WorkerTotalBusyDuration => {
                #[cfg(target_has_atomic = "64")]
                {
                    metrics.worker_total_busy_duration(worker).as_nanos() as u64
                }
                #[cfg(not(target_has_atomic = "64"))]
                {
                    0
                }
            }
            Metric::WorkerParkCount => {
                #[cfg(target_has_atomic = "64")]
                {
                    metrics.worker_park_count(worker)
                }
                #[cfg(not(target_has_atomic = "64"))]
                {
                    0
                }
            }
            Metric::WorkerParkUnparkCount => {
                #[cfg(target_has_atomic = "64")]
                {
                    metrics.worker_park_unpark_count(worker)
                }
                #[cfg(not(target_has_atomic = "64"))]
                {
                    0
                }
            }
        }
    })) {
        Ok(value) => MetricResult {
            call: result(Status::Ok, OwnedBytes::empty()),
            value,
        },
        Err(payload) => MetricResult {
            call: host_panic(&*payload),
            value: 0,
        },
    }
}

unsafe extern "C" fn block_in_place(context: *const c_void, blocking: Blocking) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    let activity = match context.owner.activity() {
        Ok(activity) => activity,
        Err(error) => return result(Status::Error, OwnedBytes::from_string(error)),
    };
    context.owner.wake_activities();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        tokio::task::block_in_place(|| unsafe { blocking.run() })
    }));
    drop(activity);
    match outcome {
        Ok(status) => result(status, OwnedBytes::empty()),
        Err(payload) => host_panic(&*payload),
    }
}

unsafe extern "C" fn abort(context: *const c_void, id: u64) {
    let context = unsafe { &*context.cast::<HandleContext>() };
    if let Some(Some(handle)) = context.owner.state.lock().unwrap().tasks.get(&id) {
        handle.abort();
    }
}

unsafe extern "C" fn is_finished(context: *const c_void, id: u64) -> bool {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match context.owner.state.lock().unwrap().tasks.get(&id) {
        Some(Some(handle)) => handle.is_finished(),
        Some(None) => false,
        None => true,
    }
}

unsafe extern "C" fn build(context: *const c_void, config: RuntimeConfig) -> BuildResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    let _activity = match context.owner.activity() {
        Ok(activity) => activity,
        Err(error) => {
            return BuildResult::error(result(Status::Error, OwnedBytes::from_string(error)));
        }
    };
    match catch_unwind(AssertUnwindSafe(|| {
        build_runtime(config, Arc::clone(&context.owner))
    })) {
        Ok(Ok((kind, handle, workers))) => {
            let runtime = Arc::new(RuntimeOwner {
                id: OnceLock::new(),
                owner: Arc::downgrade(&context.owner),
                kind: RwLock::new(kind),
            });
            let id = match context.owner.register_runtime(Arc::clone(&runtime)) {
                Ok(id) => id,
                Err(error) => {
                    let closed =
                        std::thread::spawn(move || runtime.close(Shutdown::Wait, Duration::ZERO))
                            .join();
                    return BuildResult::error(match closed {
                        Ok(Ok(())) => result(Status::Error, OwnedBytes::from_string(error)),
                        Ok(Err(error)) => {
                            result(Status::Error, OwnedBytes::from_string(error.to_string()))
                        }
                        Err(payload) => host_panic(&*payload),
                    });
                }
            };
            runtime.id.set(id).unwrap();
            let raw_handle = raw_handle(handle);
            BuildResult::success(
                unsafe {
                    RawRuntime::from_raw(Arc::into_raw(runtime).cast_mut().cast(), raw_handle)
                },
                workers,
            )
        }
        Ok(Err(error)) => BuildResult::error(result(
            Status::Error,
            OwnedBytes::from_string(error.to_string()),
        )),
        Err(payload) => BuildResult::error(host_panic(&*payload)),
    }
}

unsafe extern "C" fn clock(context: *const c_void) -> ClockSample {
    let context = unsafe { &*context.cast::<HandleContext>() };
    let origin = *CLOCK_ORIGIN.get_or_init(std::time::Instant::now);
    let realtime = instant_offset(origin, std::time::Instant::now());
    let _guard = context.handle.enter();
    let logical = instant_offset(origin, tokio::time::Instant::now().into_std());
    ClockSample { realtime, logical }
}

unsafe extern "C" fn pause(context: *const c_void) -> CallResult {
    time_call(context, tokio::time::pause)
}

unsafe extern "C" fn resume(context: *const c_void) -> CallResult {
    time_call(context, tokio::time::resume)
}

fn time_call(context: *const c_void, call: impl FnOnce()) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| {
        let _guard = context.handle.enter();
        call();
    })) {
        Ok(()) => result(Status::Ok, OwnedBytes::empty()),
        Err(payload) => host_panic(&*payload),
    }
}

unsafe extern "C" fn advance(context: *const c_void, duration: DurationParts) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| {
        let _guard = context.handle.enter();
        let mut operation = std::pin::pin!(tokio::time::advance(duration.duration()));
        let mut context = TaskContext::from_waker(std::task::Waker::noop());
        _ = operation.as_mut().poll(&mut context);
    })) {
        Ok(()) => result(Status::Ok, OwnedBytes::empty()),
        Err(payload) => host_panic(&*payload),
    }
}

unsafe extern "C" fn timer(context: *const c_void, duration: DurationParts) -> TimerResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| {
        let _guard = context.handle.enter();
        HostResource::new(
            &context.owner,
            TimeTimer {
                handle: context.handle.clone(),
                sleep: Box::pin(tokio::time::sleep(duration.duration())),
            },
        )
    })) {
        Ok(Ok(timer)) => TimerResult {
            call: result(Status::Ok, OwnedBytes::empty()),
            timer: unsafe {
                Timer::from_raw(
                    Arc::into_raw(timer).cast_mut().cast(),
                    poll_time_timer,
                    reset_time_timer,
                    time_timer_elapsed,
                    release_time_timer,
                )
            },
        },
        Ok(Err(error)) => TimerResult {
            call: result(Status::Error, OwnedBytes::from_string(error)),
            timer: Timer::empty(),
        },
        Err(payload) => TimerResult {
            call: host_panic(&*payload),
            timer: Timer::empty(),
        },
    }
}

unsafe extern "C" fn poll_time_timer(data: *mut c_void, waker: *const Waker) -> OperationPoll {
    let timer = unsafe { &*data.cast::<HostResource<TimeTimer>>() };
    timer.update_waker(unsafe { &*waker });
    let waker = unsafe { (*waker).clone_rust_waker() };
    let mut context = TaskContext::from_waker(&waker);
    match catch_unwind(AssertUnwindSafe(|| {
        timer.with_mut(|timer| timer.sleep.as_mut().poll(&mut context))
    })) {
        Ok(Ok(RustPoll::Pending)) => {
            time_poll(Poll::Pending, result(Status::Ok, OwnedBytes::empty()))
        }
        Ok(Ok(RustPoll::Ready(()))) => {
            time_poll(Poll::Ready, result(Status::Ok, OwnedBytes::empty()))
        }
        Ok(Err(error)) => time_poll(
            Poll::Ready,
            result(Status::Error, OwnedBytes::from_string(error)),
        ),
        Err(payload) => time_poll(Poll::Panicked, host_panic(&*payload)),
    }
}

unsafe extern "C" fn reset_time_timer(data: *mut c_void, duration: DurationParts) -> CallResult {
    let timer = unsafe { &*data.cast::<HostResource<TimeTimer>>() };
    match catch_unwind(AssertUnwindSafe(|| {
        timer.with_mut(|timer| {
            let _guard = timer.handle.enter();
            timer
                .sleep
                .as_mut()
                .reset(tokio::time::Instant::now() + duration.duration());
        })
    })) {
        Ok(Ok(())) => result(Status::Ok, OwnedBytes::empty()),
        Ok(Err(error)) => result(Status::Error, OwnedBytes::from_string(error)),
        Err(payload) => host_panic(&*payload),
    }
}

unsafe extern "C" fn time_timer_elapsed(data: *const c_void) -> bool {
    unsafe { &*data.cast::<HostResource<TimeTimer>>() }
        .with(|timer| timer.sleep.is_elapsed())
        .unwrap_or(true)
}

unsafe extern "C" fn release_time_timer(data: *mut c_void) {
    unsafe { Arc::from_raw(data.cast::<HostResource<TimeTimer>>()) }.release();
}

fn time_poll(state: Poll, call: CallResult) -> OperationPoll {
    OperationPoll { state, call }
}

fn instant_offset(origin: std::time::Instant, instant: std::time::Instant) -> InstantOffset {
    match instant.checked_duration_since(origin) {
        Some(duration) => InstantOffset {
            duration: DurationParts::new(duration),
            negative: 0,
        },
        None => InstantOffset {
            duration: DurationParts::new(origin.duration_since(instant)),
            negative: 1,
        },
    }
}

unsafe extern "C" fn reap_process(context: *const c_void, id: u32) -> CallResult {
    #[cfg(unix)]
    {
        let context = unsafe { &*context.cast::<HandleContext>() };
        let _guard = context.handle.enter();
        let signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::child());
        match signal {
            Ok(mut signal) => {
                context.handle.spawn(async move {
                    loop {
                        let result = unsafe {
                            libc::waitpid(id as libc::pid_t, std::ptr::null_mut(), libc::WNOHANG)
                        };
                        if result > 0 {
                            break;
                        }
                        if result < 0 {
                            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                                continue;
                            }
                            break;
                        }
                        signal.recv().await;
                    }
                });
                result(Status::Ok, OwnedBytes::empty())
            }
            Err(error) => result(Status::Error, OwnedBytes::from_string(error.to_string())),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (context, id);
        result(Status::Ok, OwnedBytes::empty())
    }
}

unsafe extern "C" fn shutdown(
    owner: *mut c_void,
    mode: Shutdown,
    seconds: u64,
    nanoseconds: u32,
) -> CallResult {
    let runtime = unsafe { &*owner.cast::<RuntimeOwner>() };
    match catch_unwind(AssertUnwindSafe(|| {
        runtime
            .close(mode, Duration::new(seconds, nanoseconds))
            .unwrap_or_else(|error| panic!("{error}"));
    })) {
        Ok(()) => result(Status::Ok, OwnedBytes::empty()),
        Err(payload) => host_panic(&*payload),
    }
}

fn build_runtime(
    config: RuntimeConfig,
    owner: Arc<OwnerState>,
) -> io::Result<(RuntimeKind, Arc<HandleContext>, usize)> {
    let thread_name = Arc::new(StringCallbackOwner(config.thread_name));
    let after_start = Arc::new(CallbackOwner(config.after_start));
    let before_stop = Arc::new(CallbackOwner(config.before_stop));
    let before_park = Arc::new(CallbackOwner(config.before_park));
    let after_unpark = Arc::new(CallbackOwner(config.after_unpark));
    let mut builder = match config.flavor {
        Flavor::CurrentThread | Flavor::Local => tokio::runtime::Builder::new_current_thread(),
        Flavor::MultiThread => tokio::runtime::Builder::new_multi_thread(),
    };

    if config.enable_io != 0 {
        builder.enable_io();
    }
    if config.enable_time != 0 {
        builder.enable_time();
    }
    if config.start_paused != 0 {
        builder.start_paused(true);
    }
    if config.worker_threads != 0 {
        builder.worker_threads(config.worker_threads);
    }
    builder.max_blocking_threads(config.max_blocking_threads);
    if config.has_thread_stack_size != 0 {
        builder.thread_stack_size(config.thread_stack_size);
    }
    if config.has_keep_alive != 0 {
        builder.thread_keep_alive(Duration::new(
            config.keep_alive_secs,
            config.keep_alive_nanos,
        ));
    }
    if config.global_queue_interval != 0 {
        builder.global_queue_interval(config.global_queue_interval);
    }
    builder.event_interval(config.event_interval);
    builder.max_io_events_per_tick(config.max_io_events_per_tick);
    if !config.name.is_empty() {
        builder.name(unsafe { config.name.as_str() });
    }

    builder.thread_name_fn(move || thread_name.call());
    if after_start.is_some() {
        builder.on_thread_start(move || after_start.call());
    }
    if before_stop.is_some() {
        builder.on_thread_stop(move || before_stop.call());
    }
    if before_park.is_some() {
        builder.on_thread_park(move || before_park.call());
    }
    if after_unpark.is_some() {
        builder.on_thread_unpark(move || after_unpark.call());
    }

    match config.flavor {
        Flavor::CurrentThread | Flavor::MultiThread => {
            let runtime = builder.build()?;
            let workers = runtime.handle().metrics().num_workers();
            let handle = handle_context(
                runtime.handle().clone(),
                Arc::clone(&owner),
                None,
                config.enable_io != 0,
            );
            Ok((RuntimeKind::Runtime(Some(runtime)), handle, workers))
        }
        Flavor::Local => {
            let runtime = builder.build_local(Default::default())?;
            let handle = runtime.handle().clone();
            let workers = handle.metrics().num_workers();
            let local = Arc::new(LocalSlot::new(runtime));
            let handle = handle_context(
                handle,
                owner,
                Some(Arc::clone(&local)),
                config.enable_io != 0,
            );
            Ok((RuntimeKind::Local(local), handle, workers))
        }
    }
}

fn handle_context(
    handle: tokio::runtime::Handle,
    owner: Arc<OwnerState>,
    local: Option<Arc<LocalSlot>>,
    io_enabled: bool,
) -> Arc<HandleContext> {
    Arc::new(HandleContext {
        handle,
        owner,
        local,
        io_enabled,
    })
}

fn raw_handle(context: Arc<HandleContext>) -> RawHandle {
    unsafe { RawHandle::from_raw(Arc::into_raw(context).cast(), &raw const RUNTIME_API) }
}

fn shutdown_runtime(runtime: tokio::runtime::Runtime, mode: Shutdown, duration: Duration) {
    match mode {
        Shutdown::Wait => drop(runtime),
        Shutdown::Background => runtime.shutdown_background(),
        Shutdown::Timeout => runtime.shutdown_timeout(duration),
    }
}

fn shutdown_local(runtime: tokio::runtime::LocalRuntime, mode: Shutdown, duration: Duration) {
    match mode {
        Shutdown::Wait => drop(runtime),
        Shutdown::Background => runtime.shutdown_background(),
        Shutdown::Timeout => runtime.shutdown_timeout(duration),
    }
}

fn result(status: Status, payload: OwnedBytes) -> CallResult {
    CallResult { status, payload }
}

fn host_panic(payload: &(dyn Any + Send)) -> CallResult {
    result(
        Status::HostPanicked,
        OwnedBytes::from_string(panic_message(payload)),
    )
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
        match self.task.poll(&waker) {
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
