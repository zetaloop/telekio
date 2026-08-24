use std::{
    any::Any,
    cell::UnsafeCell,
    collections::HashMap,
    ffi::c_void,
    future::Future as RustFuture,
    io,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    pin::Pin,
    sync::{Arc, Mutex, OnceLock},
    task::{Context as TaskContext, Poll as RustPoll},
    thread::ThreadId,
    time::Duration,
};

use telekio::{
    Blocking, BlockingTask, BuildResult, CallResult, Callback, ClockSample, DurationParts, Flavor,
    Future, InstantOffset, OperationPoll, OwnedBytes, Poll, RawHandle, RawRuntime, RuntimeApi,
    RuntimeConfig, Shutdown, Status, StringCallback, Task, Timer, TimerResult, Waker,
};

pub struct Runtime {
    runtime: tokio::runtime::Runtime,
    handle: Arc<HandleContext>,
}

struct RuntimeOwner {
    kind: RuntimeKind,
    _handle: Arc<HandleContext>,
}

enum RuntimeKind {
    Runtime(Option<tokio::runtime::Runtime>),
    Local(Arc<LocalSlot>),
}

struct HandleContext {
    handle: tokio::runtime::Handle,
    tasks: Arc<Tasks>,
    local: Option<Arc<LocalSlot>>,
}

struct LocalSlot {
    thread: ThreadId,
    runtime: UnsafeCell<Option<tokio::runtime::LocalRuntime>>,
}

#[derive(Default)]
struct Tasks {
    handles: Mutex<HashMap<u64, Option<tokio::task::AbortHandle>>>,
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
    shutdown,
};

impl Runtime {
    pub fn new() -> io::Result<Self> {
        tokio::runtime::Runtime::new().map(Self::from_tokio)
    }

    pub fn from_tokio(runtime: tokio::runtime::Runtime) -> Self {
        let handle = handle_context(runtime.handle().clone(), None);
        Self { runtime, handle }
    }

    pub fn runtime(&self) -> telekio::Handle {
        unsafe { telekio::Handle::from_abi(raw_handle(Arc::clone(&self.handle))) }
    }

    pub fn tokio(&self) -> &tokio::runtime::Runtime {
        &self.runtime
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
        !self.0.data.is_null()
    }

    fn call(&self) {
        let result = unsafe { (self.0.call)(self.0.data) };
        match result.status {
            Status::Ok => unsafe { result.payload.release() },
            Status::Panicked | Status::HostPanicked | Status::Error => {
                resume_unwind(Box::new(unsafe { result.payload.into_string() }));
            }
        }
    }
}

impl Drop for CallbackOwner {
    fn drop(&mut self) {
        unsafe { (self.0.release)(self.0.data) };
    }
}

impl StringCallbackOwner {
    fn call(&self) -> String {
        let result = unsafe { (self.0.call)(self.0.data) };
        match result.status {
            Status::Ok => unsafe { result.payload.into_string() },
            Status::Panicked | Status::HostPanicked | Status::Error => {
                resume_unwind(Box::new(unsafe { result.payload.into_string() }));
            }
        }
    }
}

impl Drop for StringCallbackOwner {
    fn drop(&mut self) {
        unsafe { (self.0.release)(self.0.data) };
    }
}

unsafe extern "C" fn retain_handle(context: *const c_void) {
    unsafe { Arc::increment_strong_count(context.cast::<HandleContext>()) };
}

unsafe extern "C" fn release_handle(context: *const c_void) {
    unsafe { Arc::decrement_strong_count(context.cast::<HandleContext>()) };
}

unsafe extern "C" fn release_runtime(owner: *mut c_void) {
    drop(unsafe { Box::from_raw(owner.cast::<RuntimeOwner>()) });
}

unsafe extern "C" fn runtime_block_on(owner: *mut c_void, future: Future) -> CallResult {
    let owner = unsafe { &*owner.cast::<RuntimeOwner>() };
    match catch_unwind(AssertUnwindSafe(|| match &owner.kind {
        RuntimeKind::Runtime(runtime) => runtime
            .as_ref()
            .expect("Tokio runtime has shut down")
            .block_on(GuestFuture(future)),
        RuntimeKind::Local(runtime) => runtime
            .with(|runtime| runtime.block_on(GuestFuture(future)))
            .unwrap_or_else(|error| panic!("{error}")),
    })) {
        Ok(status) => result(status, OwnedBytes::empty()),
        Err(payload) => host_panic(&*payload),
    }
}

unsafe extern "C" fn handle_block_on(context: *const c_void, future: Future) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| {
        context.handle.block_on(GuestFuture(future))
    })) {
        Ok(status) => result(status, OwnedBytes::empty()),
        Err(payload) => host_panic(&*payload),
    }
}

unsafe extern "C" fn spawn(context: *const c_void, id: u64, task: Task) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    let tasks = Arc::clone(&context.tasks);
    let cleanup = Arc::clone(&tasks);
    match catch_unwind(AssertUnwindSafe(|| {
        reserve(&tasks, id)?;
        let task = SendGuestTask(GuestTask::new(task));
        let handle = context.handle.spawn(async move {
            task.await;
            cleanup.handles.lock().unwrap().remove(&id);
        });
        register(&tasks, id, handle.abort_handle());
        drop(handle);
        Ok::<_, String>(())
    })) {
        Ok(Ok(())) => result(Status::Ok, OwnedBytes::empty()),
        Ok(Err(error)) => {
            context.tasks.handles.lock().unwrap().remove(&id);
            result(Status::Error, OwnedBytes::from_string(error))
        }
        Err(payload) => {
            context.tasks.handles.lock().unwrap().remove(&id);
            host_panic(&*payload)
        }
    }
}

unsafe extern "C" fn spawn_local(context: *const c_void, id: u64, task: Task) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    let tasks = Arc::clone(&context.tasks);
    let cleanup = Arc::clone(&tasks);
    let spawned = catch_unwind(AssertUnwindSafe(|| {
        reserve(&tasks, id)?;
        let local = context
            .local
            .as_ref()
            .ok_or_else(|| "spawn_local requires a LocalRuntime".to_owned())?;
        let handle = local.with(|runtime| {
            runtime.spawn_local(async move {
                GuestTask::new(task).await;
                cleanup.handles.lock().unwrap().remove(&id);
            })
        })?;
        register(&tasks, id, handle.abort_handle());
        drop(handle);
        Ok::<_, String>(())
    }));
    match spawned {
        Ok(Ok(())) => result(Status::Ok, OwnedBytes::empty()),
        Ok(Err(error)) => {
            context.tasks.handles.lock().unwrap().remove(&id);
            result(Status::Error, OwnedBytes::from_string(error))
        }
        Err(payload) => {
            context.tasks.handles.lock().unwrap().remove(&id);
            host_panic(&*payload)
        }
    }
}

unsafe extern "C" fn spawn_blocking(
    context: *const c_void,
    id: u64,
    task: BlockingTask,
) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    let tasks = Arc::clone(&context.tasks);
    let cleanup = Arc::clone(&tasks);
    let spawned = catch_unwind(AssertUnwindSafe(|| {
        reserve(&tasks, id)?;
        let task = GuestBlockingTask::new(task);
        let handle = context.handle.spawn_blocking(move || {
            task.run();
            cleanup.handles.lock().unwrap().remove(&id);
        });
        register(&tasks, id, handle.abort_handle());
        drop(handle);
        Ok::<_, String>(())
    }));
    match spawned {
        Ok(Ok(())) => result(Status::Ok, OwnedBytes::empty()),
        Ok(Err(error)) => {
            context.tasks.handles.lock().unwrap().remove(&id);
            result(Status::Error, OwnedBytes::from_string(error))
        }
        Err(payload) => {
            context.tasks.handles.lock().unwrap().remove(&id);
            host_panic(&*payload)
        }
    }
}

unsafe extern "C" fn block_in_place(_: *const c_void, blocking: Blocking) -> CallResult {
    match catch_unwind(AssertUnwindSafe(|| {
        tokio::task::block_in_place(|| unsafe { (blocking.run)(blocking.data) })
    })) {
        Ok(status) => result(status, OwnedBytes::empty()),
        Err(payload) => host_panic(&*payload),
    }
}

unsafe extern "C" fn abort(context: *const c_void, id: u64) {
    let context = unsafe { &*context.cast::<HandleContext>() };
    if let Some(Some(handle)) = context.tasks.handles.lock().unwrap().get(&id) {
        handle.abort();
    }
}

unsafe extern "C" fn is_finished(context: *const c_void, id: u64) -> bool {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match context.tasks.handles.lock().unwrap().get(&id) {
        Some(Some(handle)) => handle.is_finished(),
        Some(None) => false,
        None => true,
    }
}

unsafe extern "C" fn build(_: *const c_void, config: RuntimeConfig) -> BuildResult {
    match catch_unwind(AssertUnwindSafe(|| build_runtime(config))) {
        Ok(Ok((kind, handle, workers))) => {
            let raw_handle = raw_handle(Arc::clone(&handle));
            let owner = Box::new(RuntimeOwner {
                kind,
                _handle: handle,
            });
            BuildResult {
                call: result(Status::Ok, OwnedBytes::empty()),
                runtime: unsafe { RawRuntime::from_raw(Box::into_raw(owner).cast(), raw_handle) },
                workers,
            }
        }
        Ok(Err(error)) => BuildResult {
            call: result(Status::Error, OwnedBytes::from_string(error.to_string())),
            runtime: RawRuntime::empty(),
            workers: 0,
        },
        Err(payload) => BuildResult {
            call: host_panic(&*payload),
            runtime: RawRuntime::empty(),
            workers: 0,
        },
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
        Box::new(TimeTimer {
            handle: context.handle.clone(),
            sleep: Box::pin(tokio::time::sleep(duration.duration())),
        })
    })) {
        Ok(timer) => TimerResult {
            call: result(Status::Ok, OwnedBytes::empty()),
            timer: Timer {
                data: Box::into_raw(timer).cast(),
                poll: poll_time_timer,
                reset: reset_time_timer,
                is_elapsed: time_timer_elapsed,
                release: release_time_timer,
            },
        },
        Err(payload) => TimerResult {
            call: host_panic(&*payload),
            timer: Timer::empty(),
        },
    }
}

unsafe extern "C" fn poll_time_timer(data: *mut c_void, waker: *const Waker) -> OperationPoll {
    let timer = unsafe { &mut *data.cast::<TimeTimer>() };
    let waker = unsafe { (*waker).clone_rust_waker() };
    let mut context = TaskContext::from_waker(&waker);
    match catch_unwind(AssertUnwindSafe(|| timer.sleep.as_mut().poll(&mut context))) {
        Ok(RustPoll::Pending) => time_poll(Poll::Pending, result(Status::Ok, OwnedBytes::empty())),
        Ok(RustPoll::Ready(())) => time_poll(Poll::Ready, result(Status::Ok, OwnedBytes::empty())),
        Err(payload) => time_poll(Poll::Panicked, host_panic(&*payload)),
    }
}

unsafe extern "C" fn reset_time_timer(data: *mut c_void, duration: DurationParts) -> CallResult {
    let timer = unsafe { &mut *data.cast::<TimeTimer>() };
    match catch_unwind(AssertUnwindSafe(|| {
        let _guard = timer.handle.enter();
        timer
            .sleep
            .as_mut()
            .reset(tokio::time::Instant::now() + duration.duration());
    })) {
        Ok(()) => result(Status::Ok, OwnedBytes::empty()),
        Err(payload) => host_panic(&*payload),
    }
}

unsafe extern "C" fn time_timer_elapsed(data: *const c_void) -> bool {
    unsafe { &*data.cast::<TimeTimer>() }.sleep.is_elapsed()
}

unsafe extern "C" fn release_time_timer(data: *mut c_void) {
    drop(unsafe { Box::from_raw(data.cast::<TimeTimer>()) });
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

unsafe extern "C" fn shutdown(
    owner: *mut c_void,
    mode: Shutdown,
    seconds: u64,
    nanoseconds: u32,
) -> CallResult {
    let owner = unsafe { &mut *owner.cast::<RuntimeOwner>() };
    match catch_unwind(AssertUnwindSafe(|| {
        let duration = Duration::new(seconds, nanoseconds);
        match &mut owner.kind {
            RuntimeKind::Runtime(runtime) => {
                if let Some(runtime) = runtime.take() {
                    shutdown_runtime(runtime, mode, duration);
                }
            }
            RuntimeKind::Local(runtime) => {
                if let Some(runtime) = runtime.take().unwrap_or_else(|error| panic!("{error}")) {
                    shutdown_local(runtime, mode, duration);
                }
            }
        }
    })) {
        Ok(()) => result(Status::Ok, OwnedBytes::empty()),
        Err(payload) => host_panic(&*payload),
    }
}

fn build_runtime(config: RuntimeConfig) -> io::Result<(RuntimeKind, Arc<HandleContext>, usize)> {
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
    if config.name.len != 0 {
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
            let handle = handle_context(runtime.handle().clone(), None);
            Ok((RuntimeKind::Runtime(Some(runtime)), handle, workers))
        }
        Flavor::Local => {
            let runtime = builder.build_local(Default::default())?;
            let handle = runtime.handle().clone();
            let workers = handle.metrics().num_workers();
            let local = Arc::new(LocalSlot::new(runtime));
            let handle = handle_context(handle, Some(Arc::clone(&local)));
            Ok((RuntimeKind::Local(local), handle, workers))
        }
    }
}

fn handle_context(
    handle: tokio::runtime::Handle,
    local: Option<Arc<LocalSlot>>,
) -> Arc<HandleContext> {
    Arc::new(HandleContext {
        handle,
        tasks: Arc::new(Tasks::default()),
        local,
    })
}

fn raw_handle(context: Arc<HandleContext>) -> RawHandle {
    unsafe { RawHandle::from_raw(Arc::into_raw(context).cast(), &raw const RUNTIME_API) }
}

fn reserve(tasks: &Tasks, id: u64) -> Result<(), String> {
    let mut handles = tasks.handles.lock().unwrap();
    if handles.contains_key(&id) {
        return Err(format!("task {id} already exists"));
    }
    handles.insert(id, None);
    Ok(())
}

fn register(tasks: &Tasks, id: u64, handle: tokio::task::AbortHandle) {
    if let Some(slot) = tasks.handles.lock().unwrap().get_mut(&id) {
        *slot = Some(handle);
    }
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

struct GuestFuture(Future);

impl RustFuture for GuestFuture {
    type Output = Status;

    fn poll(self: Pin<&mut Self>, context: &mut TaskContext<'_>) -> RustPoll<Self::Output> {
        let waker = Waker::from_ref(context.waker());
        match unsafe { (self.0.poll)(self.0.data, &raw const waker) } {
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

struct SendGuestTask(GuestTask);
unsafe impl Send for SendGuestTask {}

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
        let waker = Waker::from_ref(context.waker());
        match unsafe { (self.task.poll)(self.task.data, &raw const waker) } {
            Poll::Pending => RustPoll::Pending,
            Poll::Ready => {
                self.complete = true;
                RustPoll::Ready(())
            }
            Poll::Panicked => RustPoll::Ready(()),
        }
    }
}

impl RustFuture for SendGuestTask {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut TaskContext<'_>) -> RustPoll<()> {
        Pin::new(&mut self.0).poll(context)
    }
}

impl Drop for GuestTask {
    fn drop(&mut self) {
        if !self.complete {
            unsafe { (self.task.cancel)(self.task.data) };
        }
        unsafe { (self.task.release)(self.task.data) };
    }
}

struct GuestBlockingTask {
    task: BlockingTask,
    complete: bool,
}

impl GuestBlockingTask {
    fn new(task: BlockingTask) -> Self {
        Self {
            task,
            complete: false,
        }
    }

    fn run(mut self) {
        unsafe { (self.task.run)(self.task.data) };
        self.complete = true;
    }
}

impl Drop for GuestBlockingTask {
    fn drop(&mut self) {
        if !self.complete {
            unsafe { (self.task.cancel)(self.task.data) };
        }
        unsafe { (self.task.release)(self.task.data) };
    }
}
