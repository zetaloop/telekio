use std::{
    cell::UnsafeCell,
    ffi::c_void,
    io,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::{Arc, Mutex, OnceLock, RwLock, Weak},
    thread::ThreadId,
    time::Duration,
};

use telekio::{
    Blocking, BuildResult, CallResult, Callback, Flavor, NameResult, OwnedBytes, RawHandle,
    RawRuntime, RuntimeConfig, Shutdown, Status, StringCallback, TaskCallback, TaskEvent,
    TaskIdResult, Waker, WorkerCallback,
};

use super::owner::{CallbackCleanup, Owner, OwnerState, owner_state};
use super::{RUNTIME_API, host_callback, host_panic, result};

pub struct Runtime {
    runtime: Arc<tokio::runtime::Runtime>,
    flavor: Flavor,
}

pub(super) struct RuntimeOwner {
    id: OnceLock<u64>,
    owner: Weak<OwnerState>,
    root_owner: Option<Arc<OwnerState>>,
    pub(super) kind: RwLock<RuntimeKind>,
}

pub(super) enum RuntimeKind {
    Runtime(Option<tokio::runtime::Runtime>),
    Local(Arc<LocalSlot>),
    Closed,
}

pub(super) struct HandleContext {
    pub(super) handle: tokio::runtime::Handle,
    pub(super) owner: Arc<OwnerState>,
    pub(super) flavor: Flavor,
    pub(super) local: Option<Arc<LocalSlot>>,
    pub(super) io_enabled: bool,
    worker_observer: Mutex<Option<WorkerObserver>>,
}

pub(super) struct LocalSlot {
    thread: ThreadId,
    runtime: UnsafeCell<Option<tokio::runtime::LocalRuntime>>,
}

struct WorkerObserver {
    handle: tokio::runtime::Handle,
    id: Option<u64>,
}

struct Deferred {
    state: Mutex<DeferredState>,
}

struct DeferredState {
    waker: Option<std::task::Waker>,
    cleanup: Option<CallbackCleanup>,
    woken: bool,
}

impl Deferred {
    fn wake(&self) {
        let (waker, cleanup) = {
            let mut state = self.state.lock().unwrap();
            state.woken = true;
            (state.waker.take(), state.cleanup.take())
        };
        if let Some(waker) = waker {
            waker.wake_by_ref();
        }
        drop(cleanup);
    }

    fn install(&self, cleanup: CallbackCleanup) {
        let mut state = self.state.lock().unwrap();
        if state.woken {
            drop(state);
            drop(cleanup);
        } else {
            state.cleanup = Some(cleanup);
        }
    }
}

impl std::task::Wake for Deferred {
    fn wake(self: Arc<Self>) {
        Deferred::wake(&self);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        Deferred::wake(self);
    }
}

pub(super) struct CallbackOwner(pub(super) Callback);
struct TaskCallbackOwner(TaskCallback);
struct StringCallbackOwner(StringCallback);

// LocalRuntime stays in its originating thread. HandleContext only shares the slot's
// address and checks that thread before every access to the contained runtime.
unsafe impl Send for LocalSlot {}
unsafe impl Sync for LocalSlot {}

impl Runtime {
    pub fn new() -> io::Result<Self> {
        tokio::runtime::Runtime::new().map(Self::from_tokio)
    }

    pub fn from_tokio(runtime: tokio::runtime::Runtime) -> Self {
        let flavor = match runtime.handle().runtime_flavor() {
            tokio::runtime::RuntimeFlavor::CurrentThread => Flavor::CurrentThread,
            tokio::runtime::RuntimeFlavor::MultiThread => Flavor::MultiThread,
            _ => unreachable!("unsupported Tokio runtime flavor"),
        };
        Self {
            runtime: Arc::new(runtime),
            flavor,
        }
    }

    pub fn owner(&self) -> Owner {
        Owner {
            runtime: Arc::clone(&self.runtime),
            handle: handle_context(
                self.runtime.handle().clone(),
                owner_state(),
                None,
                true,
                self.flavor,
            ),
        }
    }

    pub fn tokio(&self) -> &tokio::runtime::Runtime {
        &self.runtime
    }
}

#[doc(hidden)]
pub fn next_task_id() -> u64 {
    tokio::runtime::telekio::next_task_id()
}

#[doc(hidden)]
pub fn build_root(config: RuntimeConfig) -> BuildResult {
    match catch_unwind(AssertUnwindSafe(|| {
        let owner = owner_state();
        match build_runtime(config, Arc::clone(&owner)) {
            Ok((kind, handle, workers)) => {
                let runtime = Arc::new(RuntimeOwner {
                    id: OnceLock::new(),
                    owner: Arc::downgrade(&owner),
                    root_owner: Some(owner),
                    kind: RwLock::new(kind),
                });
                let raw_handle = raw_handle(handle);
                BuildResult::success(
                    unsafe {
                        RawRuntime::from_raw(Arc::into_raw(runtime).cast_mut().cast(), raw_handle)
                    },
                    workers,
                )
            }
            Err(error) => BuildResult::error(result(
                Status::Error,
                OwnedBytes::from_string(error.to_string()),
            )),
        }
    })) {
        Ok(result) => result,
        Err(payload) => BuildResult::error(host_panic(&*payload)),
    }
}

#[cfg(unix)]
#[doc(hidden)]
pub fn reap_process(id: u32) {
    tokio::runtime::telekio::reap_process(id);
}

impl RuntimeOwner {
    pub(super) fn owner(&self) -> Option<Arc<OwnerState>> {
        self.root_owner
            .as_ref()
            .cloned()
            .or_else(|| self.owner.upgrade())
    }

    pub(super) fn is_closed(&self) -> bool {
        matches!(&*self.kind.read().unwrap(), RuntimeKind::Closed)
    }

    pub(super) fn local_open(&self) -> bool {
        matches!(&*self.kind.read().unwrap(), RuntimeKind::Local(_))
    }

    pub(super) fn close(&self, mode: Shutdown, duration: Duration) -> io::Result<()> {
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

    pub(super) fn with<R>(
        &self,
        call: impl FnOnce(&tokio::runtime::LocalRuntime) -> R,
    ) -> Result<R, String> {
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
    pub(super) fn is_some(&self) -> bool {
        self.0.is_some()
    }

    pub(super) fn call(&self) {
        let result = self.0.call();
        match result.status {
            Status::Ok => unsafe { result.payload.release() },
            Status::Panicked | Status::HostPanicked | Status::Error => {
                resume_unwind(Box::new(unsafe { result.payload.into_string() }));
            }
        }
    }
}

impl TaskCallbackOwner {
    fn call(&self, event: TaskEvent, id: u64) {
        self.0
            .call(event, id)
            .resume("Tokio task callback panicked");
    }
}

impl Drop for WorkerObserver {
    fn drop(&mut self) {
        self.handle.telekio_remove_worker_observer(self.id);
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

pub(super) unsafe extern "C" fn release_runtime(owner: *mut c_void) -> CallResult {
    host_callback(|| {
        let runtime = unsafe { &*owner.cast::<RuntimeOwner>() };
        if runtime.root_owner.is_some() {
            drop(unsafe { Arc::from_raw(owner.cast::<RuntimeOwner>()) });
        } else if let Some(owner) = runtime.owner.upgrade() {
            let _runtime = owner.unregister_runtime(*runtime.id.get().unwrap());
        }
    })
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

pub(super) unsafe extern "C" fn trace_leaf(
    _: *const c_void,
    root: *const c_void,
    leaf: *const c_void,
) -> CallResult {
    match catch_unwind(AssertUnwindSafe(|| {
        tokio::runtime::telekio::trace_leaf(root, leaf);
    })) {
        Ok(()) => CallResult::ok(),
        Err(payload) => host_panic(&*payload),
    }
}

pub(super) unsafe extern "C" fn defer(context: *const c_void, waker: *const Waker) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| {
        let guest = unsafe { (*waker).clone_rust_waker() };
        let deferred = Arc::new(Deferred {
            state: Mutex::new(DeferredState {
                waker: Some(guest.clone()),
                cleanup: None,
                woken: false,
            }),
        });
        let waker = std::task::Waker::from(Arc::clone(&deferred));
        let Some(cleanup) = context.owner.callback(waker.clone()) else {
            guest.wake();
            return;
        };
        deferred.install(cleanup);
        tokio::runtime::telekio::defer(&waker);
    })) {
        Ok(()) => result(Status::Ok, OwnedBytes::empty()),
        Err(payload) => host_panic(&*payload),
    }
}

pub(super) unsafe extern "C" fn flavor(context: *const c_void) -> Flavor {
    unsafe { &*context.cast::<HandleContext>() }.flavor
}

pub(super) unsafe extern "C" fn id(context: *const c_void) -> u64 {
    unsafe { &*context.cast::<HandleContext>() }
        .handle
        .id()
        .telekio_value()
}

pub(super) unsafe extern "C" fn name(context: *const c_void) -> NameResult {
    match catch_unwind(AssertUnwindSafe(|| {
        unsafe { &*context.cast::<HandleContext>() }
            .handle
            .name()
            .map(str::to_owned)
    })) {
        Ok(Some(value)) => NameResult {
            call: CallResult::ok(),
            value: OwnedBytes::from_string(value),
            is_some: true,
        },
        Ok(None) => NameResult {
            call: CallResult::ok(),
            value: OwnedBytes::empty(),
            is_some: false,
        },
        Err(payload) => NameResult {
            call: host_panic(&*payload),
            value: OwnedBytes::empty(),
            is_some: false,
        },
    }
}

pub(super) unsafe extern "C" fn observe_workers(
    context: *const c_void,
    callback: WorkerCallback,
) -> CallResult {
    match catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
        let context = unsafe { &*context.cast::<HandleContext>() };
        let _activity = context.owner.activity()?;
        let mut observer = context.worker_observer.lock().unwrap();
        if observer.is_some() {
            return Err("Tokio worker observer is already installed".to_owned());
        }
        if !matches!(context.flavor, Flavor::MultiThread) {
            drop(callback);
            return Ok(());
        }
        let callback = Arc::new(callback);
        let owner = Arc::clone(&context.owner);
        let id = context
            .handle
            .telekio_add_worker_observer(Arc::new(move |worker| {
                let Some(_activity) = owner.callback(std::task::Waker::noop().clone()) else {
                    return;
                };
                callback
                    .call(worker)
                    .resume("failed to record Tokio worker thread");
            }));
        *observer = Some(WorkerObserver {
            handle: context.handle.clone(),
            id,
        });
        Ok(())
    })) {
        Ok(Ok(())) => CallResult::ok(),
        Ok(Err(error)) => result(Status::Error, OwnedBytes::from_string(error)),
        Err(payload) => host_panic(&*payload),
    }
}

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

pub(super) unsafe extern "C" fn build(
    context: *const c_void,
    config: RuntimeConfig,
) -> BuildResult {
    match catch_unwind(AssertUnwindSafe(|| {
        let context = unsafe { &*context.cast::<HandleContext>() };
        let _activity = match context.owner.activity() {
            Ok(activity) => activity,
            Err(error) => {
                return BuildResult::error(result(Status::Error, OwnedBytes::from_string(error)));
            }
        };
        match build_runtime(config, Arc::clone(&context.owner)) {
            Ok((kind, handle, workers)) => {
                let runtime = Arc::new(RuntimeOwner {
                    id: OnceLock::new(),
                    owner: Arc::downgrade(&context.owner),
                    root_owner: None,
                    kind: RwLock::new(kind),
                });
                let id = match context.owner.register_runtime(Arc::clone(&runtime)) {
                    Ok(id) => id,
                    Err(error) => {
                        let closed = std::thread::spawn(move || {
                            runtime.close(Shutdown::Wait, Duration::ZERO)
                        })
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
                        RawRuntime::from_raw(Arc::as_ptr(&runtime).cast_mut().cast(), raw_handle)
                    },
                    workers,
                )
            }
            Err(error) => BuildResult::error(result(
                Status::Error,
                OwnedBytes::from_string(error.to_string()),
            )),
        }
    })) {
        Ok(result) => result,
        Err(payload) => BuildResult::error(host_panic(&*payload)),
    }
}

pub(super) unsafe extern "C" fn reap_process_abi(_: *const c_void, id: u32) -> CallResult {
    #[cfg(unix)]
    return match catch_unwind(AssertUnwindSafe(|| reap_process(id))) {
        Ok(()) => CallResult::ok(),
        Err(payload) => host_panic(&*payload),
    };
    #[cfg(not(unix))]
    {
        let _ = id;
        CallResult::ok()
    }
}

pub(super) unsafe extern "C" fn shutdown(
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
    let task_callback = Arc::new(TaskCallbackOwner(config.task_callback));
    let mut builder = match config.flavor {
        Flavor::CurrentThread | Flavor::Local => tokio::runtime::Builder::new_current_thread(),
        Flavor::MultiThread => tokio::runtime::Builder::new_multi_thread(),
    };

    #[cfg(any(unix, windows))]
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
    #[cfg(any(unix, windows))]
    builder.max_io_events_per_tick(config.max_io_events_per_tick);
    builder.telekio_rng_seed(config.rng_one, config.rng_two);
    if config.disable_lifo_slot != 0 {
        builder.telekio_disable_lifo_slot();
    }
    if config.eager_driver_handoff != 0 {
        builder.telekio_enable_eager_driver_handoff();
    }
    if config.alternative_timer != 0 {
        builder.telekio_enable_alt_timer();
    }
    builder.telekio_unhandled_panic(config.unhandled_panic != 0);
    builder.telekio_poll_histogram(
        config.poll_histogram.kind,
        config.poll_histogram.a,
        config.poll_histogram.b,
        config.poll_histogram.c,
    );
    builder.telekio_schedule_histogram(
        config.schedule_histogram.kind,
        config.schedule_histogram.a,
        config.schedule_histogram.b,
        config.schedule_histogram.c,
    );
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
    if task_callback.0.is_some() {
        let callback = Arc::clone(&task_callback);
        builder.on_task_spawn(move |meta| {
            callback.call(TaskEvent::Spawn, meta.id().telekio_value());
        });
        let callback = Arc::clone(&task_callback);
        builder.on_before_task_poll(move |meta| {
            callback.call(TaskEvent::PollStart, meta.id().telekio_value());
        });
        let callback = Arc::clone(&task_callback);
        builder.on_after_task_poll(move |meta| {
            callback.call(TaskEvent::PollStop, meta.id().telekio_value());
        });
        builder.on_task_terminate(move |meta| {
            task_callback.call(TaskEvent::Terminate, meta.id().telekio_value());
        });
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
                config.flavor,
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
                config.flavor,
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
    flavor: Flavor,
) -> Arc<HandleContext> {
    Arc::new(HandleContext {
        handle,
        owner,
        local,
        io_enabled,
        flavor,
        worker_observer: Mutex::new(None),
    })
}

pub(super) fn raw_handle(context: Arc<HandleContext>) -> RawHandle {
    context.owner.export_handle(Arc::clone(&context));
    unsafe { RawHandle::from_raw(Arc::as_ptr(&context).cast(), &raw const RUNTIME_API) }
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
