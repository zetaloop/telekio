use crate::{
    BuildResult, Callback, ClockSample, DurationParts, InstantOffset, IoDriverResult, IoInterest,
    IoResource, IoResult, OperationPoll, RuntimeConfig, Shutdown, SignalRequest, SignalResult,
    TimerResult, WorkerCallback,
};

use std::{
    any::Any,
    cell::Cell,
    ffi::c_void,
    fmt,
    future::Future as RustFuture,
    mem::MaybeUninit,
    panic::{AssertUnwindSafe, Location, catch_unwind, resume_unwind},
    pin::Pin,
    slice,
    sync::Mutex,
    task::{Context, Poll as RustPoll, RawWaker, RawWakerVTable, Waker as RustWaker},
};

static ATTACHED: Mutex<Option<Handle>> = Mutex::new(None);

thread_local! {
    static EXECUTION: Cell<*mut ExecutionState> = const { Cell::new(std::ptr::null_mut()) };
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct RawHandle {
    pub(crate) context: *const c_void,
    pub(crate) api: *const RuntimeApi,
}

#[repr(C)]
pub struct RawRuntime {
    owner: *mut c_void,
    handle: RawHandle,
}

#[repr(C)]
pub struct RawAttachment {
    data: *mut c_void,
    enter: unsafe extern "C" fn(*mut c_void, *mut ExecutionState, GuestCall) -> CallResult,
    detach: unsafe extern "C" fn(*mut c_void) -> CallResult,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct GuestCall {
    data: *mut c_void,
    call: unsafe extern "C" fn(*mut c_void) -> CallResult,
}

#[repr(C)]
pub struct AttachResult {
    pub call: CallResult,
    pub attachment: RawAttachment,
}

#[repr(C)]
pub struct ExecutionState {
    pub task_id: u64,
    pub budget: u16,
    pub rng_one: u32,
    pub rng_two: u32,
    pub rng_active: u8,
    pub tracing: u8,
    pub forced_yields: u64,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct SourceLocation {
    file: *const u8,
    len: usize,
    line: u32,
    column: u32,
}

unsafe impl Send for SourceLocation {}
unsafe impl Sync for SourceLocation {}

pub struct Handle {
    pub(crate) raw: RawHandle,
}

pub struct Runtime {
    raw: RawRuntime,
}

unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
unsafe impl Send for RawAttachment {}
unsafe impl Sync for RawAttachment {}
unsafe impl Send for Runtime {}
unsafe impl Sync for Runtime {}

#[repr(C)]
pub struct RuntimeApi {
    pub runtime_block_on: unsafe extern "C" fn(*mut c_void, Future) -> CallResult,
    pub handle_block_on: unsafe extern "C" fn(*const c_void, Future) -> CallResult,
    pub retain_handle: unsafe extern "C" fn(*const c_void) -> CallResult,
    pub release_handle: unsafe extern "C" fn(*const c_void) -> CallResult,
    pub release_runtime: unsafe extern "C" fn(*mut c_void) -> CallResult,
    pub detach: unsafe extern "C" fn(*const c_void) -> CallResult,
    pub task_id: unsafe extern "C" fn(*const c_void) -> TaskIdResult,
    pub abort: unsafe extern "C" fn(*const c_void, u64) -> CallResult,
    pub spawn: unsafe extern "C" fn(*const c_void, Task, u64, SourceLocation) -> CallResult,
    pub spawn_local: unsafe extern "C" fn(*const c_void, Task, u64, SourceLocation) -> CallResult,
    pub spawn_blocking:
        unsafe extern "C" fn(*const c_void, BlockingTask, u64, SourceLocation) -> CallResult,
    pub task_panicked: unsafe extern "C" fn(*const c_void) -> CallResult,
    pub trace_leaf: unsafe extern "C" fn(*const c_void, *const c_void, *const c_void) -> CallResult,
    pub dump: unsafe extern "C" fn(*const c_void) -> DumpResult,
    pub block_in_place: unsafe extern "C" fn(*const c_void, Blocking) -> CallResult,
    pub build: unsafe extern "C" fn(*const c_void, RuntimeConfig) -> BuildResult,
    pub clock: unsafe extern "C" fn(*const c_void) -> ClockResult,
    pub pause: unsafe extern "C" fn(*const c_void) -> CallResult,
    pub resume: unsafe extern "C" fn(*const c_void) -> CallResult,
    pub advance: unsafe extern "C" fn(*const c_void, DurationParts) -> CallResult,
    pub timer: unsafe extern "C" fn(*const c_void, InstantOffset) -> TimerResult,
    pub register_io: unsafe extern "C" fn(*const c_void, IoResource, IoInterest) -> IoResult,
    pub register_io_driver:
        unsafe extern "C" fn(*const c_void, IoResource, Callback) -> IoDriverResult,
    pub signal: unsafe extern "C" fn(*const c_void, SignalRequest) -> SignalResult,
    pub reap_process: unsafe extern "C" fn(*const c_void, u32) -> CallResult,
    pub shutdown: unsafe extern "C" fn(*mut c_void, Shutdown, u64, u32) -> CallResult,
    pub defer: unsafe extern "C" fn(*const c_void, *const Waker) -> CallResult,
    pub metric: unsafe extern "C" fn(*const c_void, Metric, usize, usize) -> MetricResult,
    pub flavor: unsafe extern "C" fn(*const c_void) -> crate::Flavor,
    pub id: unsafe extern "C" fn(*const c_void) -> u64,
    pub name: unsafe extern "C" fn(*const c_void) -> NameResult,
    pub observe_workers: unsafe extern "C" fn(*const c_void, WorkerCallback) -> CallResult,
}

#[repr(C)]
pub struct CallResult {
    pub status: Status,
    pub payload: OwnedBytes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Status {
    Ok,
    Panicked,
    HostPanicked,
    Error,
}

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum Metric {
    GlobalQueueDepth,
    NumAliveTasks,
    SpawnedTasksCount,
    BudgetForcedYieldCount,
    WorkerTotalBusyDuration,
    WorkerParkCount,
    WorkerParkUnparkCount,
    NumWorkers,
    NumBlockingThreads,
    NumIdleBlockingThreads,
    WorkerLocalQueueDepth,
    BlockingQueueDepth,
    RemoteScheduleCount,
    WorkerNoopCount,
    WorkerStealCount,
    WorkerStealOperations,
    WorkerPollCount,
    WorkerLocalScheduleCount,
    WorkerOverflowCount,
    PollTimeHistogramEnabled,
    PollTimeHistogramNumBuckets,
    PollTimeHistogramRangeStart,
    PollTimeHistogramRangeEnd,
    PollTimeHistogramBucketCount,
    WorkerMeanPollTime,
    ScheduleLatencyHistogramEnabled,
    ScheduleLatencyHistogramNumBuckets,
    ScheduleLatencyHistogramRangeStart,
    ScheduleLatencyHistogramRangeEnd,
    ScheduleLatencyHistogramBucketCount,
    IoDriverFdRegisteredCount,
    IoDriverFdDeregisteredCount,
    IoDriverReadyCount,
    CurrentWorkerIndex,
}

#[repr(C)]
pub struct MetricResult {
    pub call: CallResult,
    pub value: u64,
}

#[repr(C)]
pub struct TaskIdResult {
    pub call: CallResult,
    pub value: u64,
}

#[repr(C)]
pub struct NameResult {
    pub call: CallResult,
    pub value: OwnedBytes,
    pub is_some: bool,
}

#[repr(C)]
pub struct BoolResult {
    pub call: CallResult,
    pub value: bool,
}

#[repr(C)]
pub struct ClockResult {
    pub call: CallResult,
    pub value: ClockSample,
}

#[repr(C)]
pub struct OwnedBytes {
    data: *mut u8,
    len: usize,
    release: unsafe extern "C" fn(*mut u8, usize),
}

#[repr(C)]
pub struct Future {
    data: *mut c_void,
    poll: unsafe extern "C" fn(*mut c_void, *mut ExecutionState, *const Waker) -> Poll,
}

#[repr(C)]
pub struct DumpOperation {
    data: *mut c_void,
    poll: unsafe extern "C" fn(*mut c_void, *const Waker) -> OperationPoll,
    release: unsafe extern "C" fn(*mut c_void) -> CallResult,
}

#[repr(C)]
pub struct DumpResult {
    pub call: CallResult,
    pub dump: DumpOperation,
}

unsafe impl Send for DumpOperation {}

#[repr(C)]
pub struct Task {
    data: *mut c_void,
    poll: unsafe extern "C" fn(*mut c_void, *mut ExecutionState, *const Waker) -> TaskPoll,
    cancel: unsafe extern "C" fn(*mut c_void, *mut ExecutionState) -> CallResult,
    release: unsafe extern "C" fn(*mut c_void) -> CallResult,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct TaskPoll {
    state: Poll,
    duration_nanos: u64,
    measured: u8,
}

#[repr(C)]
pub struct BlockingTask {
    data: *mut c_void,
    run: unsafe extern "C" fn(*mut c_void, *mut ExecutionState) -> CallResult,
    cancel: unsafe extern "C" fn(*mut c_void, *mut ExecutionState) -> CallResult,
    release: unsafe extern "C" fn(*mut c_void) -> CallResult,
}

unsafe impl Send for BlockingTask {}

#[repr(C)]
pub struct Blocking {
    data: *mut c_void,
    run: unsafe extern "C" fn(*mut c_void) -> Status,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Poll {
    Pending,
    Ready,
    Panicked,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Waker {
    data: *const c_void,
    clone: unsafe extern "C" fn(*const c_void, *mut Waker) -> CallResult,
    wake: unsafe extern "C" fn(*const c_void) -> CallResult,
    wake_by_ref: unsafe extern "C" fn(*const c_void) -> CallResult,
    release: unsafe extern "C" fn(*const c_void) -> CallResult,
}

unsafe impl Send for Waker {}
unsafe impl Sync for Waker {}

struct FutureState<F: RustFuture> {
    future: Option<Pin<Box<F>>>,
    result: Option<Result<F::Output, Box<dyn Any + Send>>>,
}

impl RawAttachment {
    pub const fn empty() -> Self {
        Self {
            data: std::ptr::null_mut(),
            enter: enter_empty,
            detach: detach_empty,
        }
    }

    /// # Safety
    ///
    /// `data` and the callbacks must describe one owned guest execution
    /// context that remains valid until `detach` is called.
    #[doc(hidden)]
    pub const unsafe fn from_raw(
        data: *mut c_void,
        enter: unsafe extern "C" fn(*mut c_void, *mut ExecutionState, GuestCall) -> CallResult,
        detach: unsafe extern "C" fn(*mut c_void) -> CallResult,
    ) -> Self {
        Self {
            data,
            enter,
            detach,
        }
    }

    /// # Safety
    ///
    /// The plugin defining these callbacks must remain loaded.
    #[doc(hidden)]
    pub unsafe fn enter(&self, state: *mut ExecutionState, call: GuestCall) -> CallResult {
        unsafe { (self.enter)(self.data, state, call) }
    }

    /// # Safety
    ///
    /// The plugin defining these callbacks must remain loaded, and its owner
    /// must have completed shutdown.
    #[doc(hidden)]
    pub unsafe fn detach(&self) -> CallResult {
        unsafe { (self.detach)(self.data) }
    }
}

impl GuestCall {
    /// # Safety
    ///
    /// `data` must remain valid for one synchronous invocation of `call`.
    #[doc(hidden)]
    pub const unsafe fn from_raw(
        data: *mut c_void,
        call: unsafe extern "C" fn(*mut c_void) -> CallResult,
    ) -> Self {
        Self { data, call }
    }

    /// # Safety
    ///
    /// The backing call state must still be valid and may be invoked once.
    #[doc(hidden)]
    pub unsafe fn invoke(self) -> CallResult {
        unsafe { (self.call)(self.data) }
    }
}

impl SourceLocation {
    #[track_caller]
    #[doc(hidden)]
    pub fn caller() -> Self {
        Self::from_location(Location::caller())
    }

    #[doc(hidden)]
    pub fn from_location(location: &'static Location<'static>) -> Self {
        let file = location.file();
        Self {
            file: file.as_ptr(),
            len: file.len(),
            line: location.line(),
            column: location.column(),
        }
    }

    /// # Safety
    ///
    /// The artifact defining this descriptor must remain loaded.
    #[doc(hidden)]
    pub unsafe fn file(&self) -> &str {
        unsafe { str::from_utf8_unchecked(slice::from_raw_parts(self.file, self.len)) }
    }

    #[doc(hidden)]
    pub const fn line(&self) -> u32 {
        self.line
    }

    #[doc(hidden)]
    pub const fn column(&self) -> u32 {
        self.column
    }
}

#[doc(hidden)]
pub fn execution_state() -> *mut ExecutionState {
    EXECUTION.get()
}

/// # Safety
///
/// `state` must remain exclusively available on the current thread for the
/// duration of `call`.
#[doc(hidden)]
pub unsafe fn with_execution_state<R>(state: *mut ExecutionState, call: impl FnOnce() -> R) -> R {
    struct Reset(*mut ExecutionState);

    impl Drop for Reset {
        fn drop(&mut self) {
            EXECUTION.set(self.0);
        }
    }

    assert!(!state.is_null(), "Tokio execution state is missing");
    let previous = EXECUTION.replace(state);
    let _reset = Reset(previous);
    call()
}

impl RawHandle {
    pub const fn is_empty(self) -> bool {
        self.context.is_null() || self.api.is_null()
    }

    pub const fn empty() -> Self {
        Self {
            context: std::ptr::null(),
            api: std::ptr::null(),
        }
    }

    /// # Safety
    ///
    /// `context` and `api` must form one valid host handle reference.
    pub const unsafe fn from_raw(context: *const c_void, api: *const RuntimeApi) -> Self {
        Self { context, api }
    }
}

impl Future {
    /// # Safety
    ///
    /// `data` must remain valid and exclusively accessible for every call to
    /// `poll`.
    #[doc(hidden)]
    pub const unsafe fn from_raw(
        data: *mut c_void,
        poll: unsafe extern "C" fn(*mut c_void, *mut ExecutionState, *const Waker) -> Poll,
    ) -> Self {
        Self { data, poll }
    }

    #[doc(hidden)]
    pub fn poll(&mut self, state: &mut ExecutionState, waker: &Waker) -> Poll {
        unsafe { (self.poll)(self.data, state, waker) }
    }
}

impl TaskPoll {
    #[doc(hidden)]
    pub const fn new(state: Poll, duration_nanos: u64) -> Self {
        Self {
            state,
            duration_nanos,
            measured: 1,
        }
    }

    #[doc(hidden)]
    pub const fn unmeasured(state: Poll) -> Self {
        Self {
            state,
            duration_nanos: 0,
            measured: 0,
        }
    }

    #[doc(hidden)]
    pub const fn state(self) -> Poll {
        self.state
    }

    #[doc(hidden)]
    pub const fn duration_nanos(self) -> Option<u64> {
        if self.measured == 0 {
            None
        } else {
            Some(self.duration_nanos)
        }
    }
}

impl DumpOperation {
    pub const fn empty() -> Self {
        Self {
            data: std::ptr::null_mut(),
            poll: poll_dump_empty,
            release: release_dump_empty,
        }
    }

    /// # Safety
    ///
    /// `data` and the callbacks must describe one owned dump operation.
    #[doc(hidden)]
    pub const unsafe fn from_raw(
        data: *mut c_void,
        poll: unsafe extern "C" fn(*mut c_void, *const Waker) -> OperationPoll,
        release: unsafe extern "C" fn(*mut c_void) -> CallResult,
    ) -> Self {
        Self {
            data,
            poll,
            release,
        }
    }

    #[doc(hidden)]
    pub fn poll(&mut self, waker: &Waker) -> RustPoll<Vec<u8>> {
        let result = unsafe { (self.poll)(self.data, waker) };
        match result.state {
            crate::Poll::Pending => {
                result.call.resume("failed to poll Tokio runtime dump");
                RustPoll::Pending
            }
            crate::Poll::Ready => match result.call.status {
                Status::Ok => RustPoll::Ready(unsafe { result.call.payload.into_vec() }),
                Status::Error | Status::Panicked | Status::HostPanicked => {
                    result.call.resume("failed to poll Tokio runtime dump");
                    unreachable!()
                }
            },
            crate::Poll::Panicked => {
                result.call.resume("Tokio runtime dump panicked");
                unreachable!()
            }
        }
    }
}

impl Drop for DumpOperation {
    fn drop(&mut self) {
        unsafe { (self.release)(self.data) }.resume("failed to release Tokio runtime dump");
    }
}

impl Task {
    /// # Safety
    ///
    /// `data` and the callbacks must describe one owned task state. `cancel`
    /// may be called at most once before this value is dropped. The state must
    /// be `Send` when passed to `Handle::spawn`; non-`Send` state may only be
    /// passed to `Handle::spawn_local`.
    #[doc(hidden)]
    pub const unsafe fn from_raw(
        data: *mut c_void,
        poll: unsafe extern "C" fn(*mut c_void, *mut ExecutionState, *const Waker) -> TaskPoll,
        cancel: unsafe extern "C" fn(*mut c_void, *mut ExecutionState) -> CallResult,
        release: unsafe extern "C" fn(*mut c_void) -> CallResult,
    ) -> Self {
        Self {
            data,
            poll,
            cancel,
            release,
        }
    }

    #[doc(hidden)]
    pub fn poll(&mut self, state: &mut ExecutionState, waker: &Waker) -> TaskPoll {
        unsafe { (self.poll)(self.data, state, waker) }
    }

    /// # Safety
    ///
    /// The task must not have been cancelled or completed already.
    #[doc(hidden)]
    pub unsafe fn cancel(&mut self, state: *mut ExecutionState) {
        unsafe { (self.cancel)(self.data, state) }.resume("failed to cancel Tokio task");
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        unsafe { (self.release)(self.data) }.resume("failed to release Tokio task");
    }
}

impl BlockingTask {
    /// # Safety
    ///
    /// `data` and the callbacks must describe one owned, `Send` blocking task
    /// state. Exactly one of `run` or `cancel` may be called before this value
    /// is dropped.
    #[doc(hidden)]
    pub const unsafe fn from_raw(
        data: *mut c_void,
        run: unsafe extern "C" fn(*mut c_void, *mut ExecutionState) -> CallResult,
        cancel: unsafe extern "C" fn(*mut c_void, *mut ExecutionState) -> CallResult,
        release: unsafe extern "C" fn(*mut c_void) -> CallResult,
    ) -> Self {
        Self {
            data,
            run,
            cancel,
            release,
        }
    }

    /// # Safety
    ///
    /// This task must not have been run or cancelled already.
    #[doc(hidden)]
    pub unsafe fn run(&mut self, state: *mut ExecutionState) {
        unsafe { (self.run)(self.data, state) }.resume("failed to run Tokio blocking task");
    }

    /// # Safety
    ///
    /// This task must not have been run or cancelled already.
    #[doc(hidden)]
    pub unsafe fn cancel(&mut self, state: *mut ExecutionState) {
        unsafe { (self.cancel)(self.data, state) }.resume("failed to cancel Tokio blocking task");
    }
}

impl Drop for BlockingTask {
    fn drop(&mut self) {
        unsafe { (self.release)(self.data) }.resume("failed to release Tokio blocking task");
    }
}

impl Blocking {
    /// # Safety
    ///
    /// `data` must remain valid for one call to `run`.
    #[doc(hidden)]
    pub const unsafe fn from_raw(
        data: *mut c_void,
        run: unsafe extern "C" fn(*mut c_void) -> Status,
    ) -> Self {
        Self { data, run }
    }

    /// # Safety
    ///
    /// The backing call state must still be valid and may be invoked once.
    #[doc(hidden)]
    pub unsafe fn run(self) -> Status {
        unsafe { (self.run)(self.data) }
    }
}

impl RawRuntime {
    pub const fn is_empty(&self) -> bool {
        self.owner.is_null() || self.handle.is_empty()
    }

    pub const fn empty() -> Self {
        Self {
            owner: std::ptr::null_mut(),
            handle: RawHandle::empty(),
        }
    }

    /// # Safety
    ///
    /// `owner` must be one runtime owner associated with `handle`.
    pub const unsafe fn from_raw(owner: *mut c_void, handle: RawHandle) -> Self {
        Self { owner, handle }
    }
}

#[doc(hidden)]
pub fn install_handle(handle: Handle) -> Result<(), Handle> {
    let mut attached = ATTACHED.lock().unwrap();
    if attached.is_some() {
        Err(handle)
    } else {
        *attached = Some(handle);
        Ok(())
    }
}

#[cfg(feature = "guest")]
unsafe extern "C" {
    fn telekio_guest_context(raw: RawHandle) -> AttachResult;
}

/// # Safety
///
/// `raw` must be one owned host handle reference returned by its host API.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telekio_guest_attach(raw: RawHandle) -> AttachResult {
    #[cfg(feature = "guest")]
    {
        unsafe { telekio_guest_context(raw) }
    }
    #[cfg(not(feature = "guest"))]
    {
        drop(unsafe { Handle::from_abi(raw) });
        crate::require_package("This package");
        AttachResult {
            call: CallResult::error("Telekio guest is not enabled"),
            attachment: RawAttachment::empty(),
        }
    }
}

/// # Safety
///
/// The guest execution context must have been released, and the attached host
/// owner must have completed shutdown.
#[doc(hidden)]
pub unsafe fn detach_attached() -> CallResult {
    match catch_unwind(AssertUnwindSafe(|| {
        let handle = ATTACHED
            .lock()
            .unwrap()
            .take()
            .ok_or("Telekio runtime is not attached")?;
        let result = unsafe { ((*handle.raw.api).detach)(handle.raw.context) };
        if result.status == Status::Ok {
            std::mem::forget(handle);
        } else {
            *ATTACHED.lock().unwrap() = Some(handle);
        }
        Ok::<_, &str>(result)
    })) {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => CallResult::error(error),
        Err(payload) => CallResult::panicked(&*payload),
    }
}

#[doc(hidden)]
pub fn attached() -> Handle {
    if let Some(handle) = ATTACHED.lock().unwrap().as_ref() {
        return handle.clone();
    }
    #[cfg(feature = "guest")]
    {
        unsafe extern "C-unwind" {
            fn telekio_default_handle() -> RawHandle;
        }
        let raw = unsafe { telekio_default_handle() };
        if !raw.is_empty() {
            let handle = unsafe { Handle::from_abi(raw) };
            if install_handle(handle).is_ok() {
                return ATTACHED.lock().unwrap().as_ref().unwrap().clone();
            }
        }
    }
    panic!("Telekio runtime is not attached")
}

impl Handle {
    /// # Safety
    ///
    /// `raw` must be one owned host handle reference returned by its host API.
    pub const unsafe fn from_abi(raw: RawHandle) -> Self {
        Self { raw }
    }

    pub fn into_abi(self) -> RawHandle {
        let raw = self.raw;
        std::mem::forget(self);
        raw
    }

    pub fn block_on<F: RustFuture>(&self, future: F) -> F::Output {
        block_on(future, |future| unsafe {
            ((*self.raw.api).handle_block_on)(self.raw.context, future)
        })
    }

    #[doc(hidden)]
    pub fn register_io_driver(&self, resource: IoResource, callback: Callback) -> IoDriverResult {
        unsafe { ((*self.raw.api).register_io_driver)(self.raw.context, resource, callback) }
    }

    #[doc(hidden)]
    pub fn reap_process(&self, id: u32) -> CallResult {
        unsafe { ((*self.raw.api).reap_process)(self.raw.context, id) }
    }

    #[doc(hidden)]
    pub fn next_task_id(&self) -> u64 {
        let result = unsafe { ((*self.raw.api).task_id)(self.raw.context) };
        result.call.resume("failed to allocate Tokio task ID");
        result.value
    }

    #[doc(hidden)]
    pub fn abort(&self, id: u64) -> CallResult {
        unsafe { ((*self.raw.api).abort)(self.raw.context, id) }
    }

    pub fn spawn(&self, task: Task, id: u64, location: SourceLocation) -> CallResult {
        unsafe { ((*self.raw.api).spawn)(self.raw.context, task, id, location) }
    }

    #[doc(hidden)]
    pub fn spawn_local(&self, task: Task, id: u64, location: SourceLocation) -> CallResult {
        unsafe { ((*self.raw.api).spawn_local)(self.raw.context, task, id, location) }
    }

    #[doc(hidden)]
    pub fn spawn_blocking(
        &self,
        task: BlockingTask,
        id: u64,
        location: SourceLocation,
    ) -> CallResult {
        unsafe { ((*self.raw.api).spawn_blocking)(self.raw.context, task, id, location) }
    }

    #[doc(hidden)]
    pub fn task_panicked(&self) -> CallResult {
        unsafe { ((*self.raw.api).task_panicked)(self.raw.context) }
    }

    /// # Safety
    ///
    /// `root` and `leaf` must be code addresses in the calling artifact.
    #[doc(hidden)]
    pub unsafe fn trace_leaf(&self, root: *const c_void, leaf: *const c_void) -> CallResult {
        unsafe { ((*self.raw.api).trace_leaf)(self.raw.context, root, leaf) }
    }

    #[doc(hidden)]
    pub fn dump(&self) -> DumpResult {
        unsafe { ((*self.raw.api).dump)(self.raw.context) }
    }

    #[doc(hidden)]
    pub fn defer(&self, waker: &Waker) -> CallResult {
        unsafe { ((*self.raw.api).defer)(self.raw.context, waker) }
    }

    #[doc(hidden)]
    #[track_caller]
    pub fn metric(&self, metric: Metric, worker: usize) -> u64 {
        self.metric_bucket(metric, worker, 0)
    }

    #[doc(hidden)]
    #[track_caller]
    pub fn metric_bucket(&self, metric: Metric, worker: usize, bucket: usize) -> u64 {
        let result = unsafe { ((*self.raw.api).metric)(self.raw.context, metric, worker, bucket) };
        result.call.into_io_result().unwrap();
        result.value
    }

    #[doc(hidden)]
    pub fn flavor(&self) -> crate::Flavor {
        unsafe { ((*self.raw.api).flavor)(self.raw.context) }
    }

    #[doc(hidden)]
    pub fn id(&self) -> u64 {
        unsafe { ((*self.raw.api).id)(self.raw.context) }
    }

    #[doc(hidden)]
    pub fn name(&self) -> Option<String> {
        let result = unsafe { ((*self.raw.api).name)(self.raw.context) };
        result.call.resume("failed to read Tokio runtime name");
        if result.is_some {
            Some(unsafe { result.value.into_string() })
        } else {
            unsafe { result.value.release() };
            None
        }
    }

    #[doc(hidden)]
    pub fn observe_workers(&self, callback: WorkerCallback) -> CallResult {
        unsafe { ((*self.raw.api).observe_workers)(self.raw.context, callback) }
    }

    pub fn block_in_place(&self, blocking: Blocking) -> CallResult {
        unsafe { ((*self.raw.api).block_in_place)(self.raw.context, blocking) }
    }

    #[doc(hidden)]
    pub fn build(&self, config: RuntimeConfig) -> BuildResult {
        unsafe { ((*self.raw.api).build)(self.raw.context, config) }
    }
}

impl Clone for Handle {
    fn clone(&self) -> Self {
        unsafe { ((*self.raw.api).retain_handle)(self.raw.context) }
            .resume("failed to retain Tokio handle");
        Self { raw: self.raw }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { ((*self.raw.api).release_handle)(self.raw.context) }
            .resume("failed to release Tokio handle");
    }
}

impl Runtime {
    /// # Safety
    ///
    /// `raw` must be one owned runtime returned by its host API.
    pub const unsafe fn from_abi(raw: RawRuntime) -> Self {
        Self { raw }
    }

    pub fn handle(&self) -> Handle {
        unsafe { ((*self.raw.handle.api).retain_handle)(self.raw.handle.context) }
            .resume("failed to retain Tokio handle");
        Handle {
            raw: self.raw.handle,
        }
    }

    pub fn block_on<F: RustFuture>(&self, future: F) -> F::Output {
        block_on(future, |future| unsafe {
            ((*self.raw.handle.api).runtime_block_on)(self.raw.owner, future)
        })
    }

    #[doc(hidden)]
    pub fn shutdown(&self, mode: Shutdown, seconds: u64, nanoseconds: u32) -> CallResult {
        unsafe { ((*self.raw.handle.api).shutdown)(self.raw.owner, mode, seconds, nanoseconds) }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe { ((*self.raw.handle.api).release_runtime)(self.raw.owner) }
            .resume("failed to release Tokio runtime");
        unsafe { ((*self.raw.handle.api).release_handle)(self.raw.handle.context) }
            .resume("failed to release Tokio handle");
    }
}

impl CallResult {
    #[doc(hidden)]
    pub fn ok() -> Self {
        Self {
            status: Status::Ok,
            payload: OwnedBytes::empty(),
        }
    }

    #[doc(hidden)]
    pub fn error(message: &str) -> Self {
        Self {
            status: Status::Error,
            payload: OwnedBytes::from_string(message.to_owned()),
        }
    }

    #[doc(hidden)]
    pub fn panicked(payload: &(dyn Any + Send)) -> Self {
        Self {
            status: Status::Panicked,
            payload: OwnedBytes::from_string(panic_message(payload)),
        }
    }

    #[doc(hidden)]
    pub fn resume(self, context: &str) {
        self.into_io_result().expect(context);
    }

    #[doc(hidden)]
    pub fn into_io_result(self) -> std::io::Result<()> {
        match self.status {
            Status::Ok => {
                unsafe { self.payload.release() };
                Ok(())
            }
            Status::Error => Err(std::io::Error::other(unsafe { self.payload.into_string() })),
            Status::Panicked | Status::HostPanicked => {
                resume_unwind(Box::new(unsafe { self.payload.into_string() }))
            }
        }
    }
}

impl OwnedBytes {
    pub fn empty() -> Self {
        Self {
            data: std::ptr::null_mut(),
            len: 0,
            release: release_bytes,
        }
    }

    pub fn from_string(value: String) -> Self {
        Self::from_vec(value.into_bytes())
    }

    #[doc(hidden)]
    pub fn from_vec(value: Vec<u8>) -> Self {
        let bytes = value.into_boxed_slice();
        let len = bytes.len();
        if len == 0 {
            return Self::empty();
        }
        Self {
            data: Box::into_raw(bytes).cast(),
            len,
            release: release_bytes,
        }
    }

    /// # Safety
    ///
    /// This descriptor must still own the allocation supplied by its producer.
    #[doc(hidden)]
    pub unsafe fn release(self) {
        unsafe { (self.release)(self.data, self.len) };
    }

    /// # Safety
    ///
    /// This descriptor must contain one uniquely owned UTF-8 allocation.
    #[doc(hidden)]
    pub unsafe fn into_string(self) -> String {
        String::from_utf8(unsafe { self.into_vec() })
            .unwrap_or_else(|_| "host Tokio runtime panicked".to_owned())
    }

    /// # Safety
    ///
    /// This descriptor must contain one uniquely owned allocation.
    #[doc(hidden)]
    pub unsafe fn into_vec(self) -> Vec<u8> {
        let bytes = if self.len == 0 {
            Vec::new()
        } else {
            unsafe { slice::from_raw_parts(self.data, self.len) }.to_vec()
        };
        unsafe { self.release() };
        bytes
    }
}

impl fmt::Debug for Handle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Handle")
            .field("context", &self.raw.context)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for Runtime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Runtime")
            .field("owner", &self.raw.owner)
            .field("handle", &self.raw.handle.context)
            .finish_non_exhaustive()
    }
}

fn block_on<F: RustFuture>(future: F, call: impl FnOnce(Future) -> CallResult) -> F::Output {
    let mut state = FutureState {
        future: Some(Box::pin(future)),
        result: None,
    };
    let result = call(unsafe { Future::from_raw((&raw mut state).cast(), poll_future::<F>) });
    match (result.status, state.result) {
        (Status::Ok, Some(Ok(output))) => {
            unsafe { result.payload.release() };
            output
        }
        (Status::Panicked, Some(Err(payload))) => {
            unsafe { result.payload.release() };
            resume_unwind(payload)
        }
        (Status::HostPanicked, _) => {
            resume_unwind(Box::new(unsafe { result.payload.into_string() }))
        }
        (Status::Error, _) => {
            panic!("host Tokio runtime operation failed: {}", unsafe {
                result.payload.into_string()
            })
        }
        _ => {
            unsafe { result.payload.release() };
            panic!("host Tokio runtime returned an invalid block_on result")
        }
    }
}

unsafe extern "C" fn release_bytes(data: *mut u8, len: usize) {
    if !data.is_null() {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(data, len)) });
    }
}

unsafe extern "C" fn poll_future<F: RustFuture>(
    data: *mut c_void,
    execution: *mut ExecutionState,
    waker: *const Waker,
) -> Poll {
    let state = unsafe { &mut *data.cast::<FutureState<F>>() };
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        with_execution_state(execution, || {
            let waker = (*waker).clone_rust_waker();
            let mut context = Context::from_waker(&waker);
            let poll = state
                .future
                .as_mut()
                .expect("completed future was polled")
                .as_mut()
                .poll(&mut context);
            if poll.is_ready() {
                state.future = None;
            }
            poll
        })
    }));
    match result {
        Ok(RustPoll::Pending) => Poll::Pending,
        Ok(RustPoll::Ready(output)) => {
            state.result = Some(Ok(output));
            Poll::Ready
        }
        Err(payload) => {
            state.result = Some(Err(payload));
            Poll::Panicked
        }
    }
}

impl Waker {
    /// # Safety
    ///
    /// `waker` must outlive every use of the returned borrowed descriptor.
    #[doc(hidden)]
    pub unsafe fn from_ref(waker: &RustWaker) -> Self {
        Self {
            data: (waker as *const RustWaker).cast(),
            clone: clone_borrowed,
            wake: wake_borrowed,
            wake_by_ref: wake_borrowed,
            release: release_borrowed,
        }
    }

    /// # Safety
    ///
    /// This descriptor must contain valid waker callbacks and state.
    #[doc(hidden)]
    pub unsafe fn clone_rust_waker(&self) -> RustWaker {
        let mut owned = MaybeUninit::uninit();
        unsafe { (self.clone)(self.data, owned.as_mut_ptr()) }
            .resume("failed to clone Tokio waker");
        let owned = unsafe { owned.assume_init() };
        let raw = RawWaker::new(Box::into_raw(Box::new(owned)).cast(), &RAW_WAKER_VTABLE);
        unsafe { RustWaker::from_raw(raw) }
    }
}

unsafe extern "C" fn clone_borrowed(data: *const c_void, output: *mut Waker) -> CallResult {
    callback(|| {
        let waker = unsafe { &*data.cast::<RustWaker>() };
        unsafe { output.write(owned_waker(waker.clone())) };
    })
}

unsafe extern "C" fn wake_borrowed(data: *const c_void) -> CallResult {
    callback(|| unsafe { &*data.cast::<RustWaker>() }.wake_by_ref())
}

unsafe extern "C" fn release_borrowed(_: *const c_void) -> CallResult {
    CallResult::ok()
}

fn owned_waker(waker: RustWaker) -> Waker {
    Waker {
        data: Box::into_raw(Box::new(waker)).cast(),
        clone: clone_owned,
        wake: wake_owned,
        wake_by_ref: wake_by_ref_owned,
        release: release_owned,
    }
}

unsafe extern "C" fn clone_owned(data: *const c_void, output: *mut Waker) -> CallResult {
    callback(|| {
        let waker = unsafe { &*data.cast::<RustWaker>() };
        unsafe { output.write(owned_waker(waker.clone())) };
    })
}

unsafe extern "C" fn wake_owned(data: *const c_void) -> CallResult {
    callback(|| unsafe { Box::from_raw(data.cast_mut().cast::<RustWaker>()) }.wake())
}

unsafe extern "C" fn wake_by_ref_owned(data: *const c_void) -> CallResult {
    callback(|| unsafe { &*data.cast::<RustWaker>() }.wake_by_ref())
}

unsafe extern "C" fn release_owned(data: *const c_void) -> CallResult {
    callback(|| drop(unsafe { Box::from_raw(data.cast_mut().cast::<RustWaker>()) }))
}

static RAW_WAKER_VTABLE: RawWakerVTable =
    RawWakerVTable::new(raw_clone, raw_wake, raw_wake_by_ref, raw_drop);

unsafe fn raw_clone(data: *const ()) -> RawWaker {
    let waker = unsafe { &*data.cast::<Waker>() };
    let mut cloned = MaybeUninit::uninit();
    unsafe { (waker.clone)(waker.data, cloned.as_mut_ptr()) }.resume("failed to clone Tokio waker");
    let cloned = unsafe { cloned.assume_init() };
    RawWaker::new(Box::into_raw(Box::new(cloned)).cast(), &RAW_WAKER_VTABLE)
}

unsafe fn raw_wake(data: *const ()) {
    let waker = unsafe { Box::from_raw(data.cast_mut().cast::<Waker>()) };
    unsafe { (waker.wake)(waker.data) }.resume("failed to wake Tokio task");
}

unsafe fn raw_wake_by_ref(data: *const ()) {
    let waker = unsafe { &*data.cast::<Waker>() };
    unsafe { (waker.wake_by_ref)(waker.data) }.resume("failed to wake Tokio task");
}

unsafe fn raw_drop(data: *const ()) {
    let waker = unsafe { Box::from_raw(data.cast_mut().cast::<Waker>()) };
    unsafe { (waker.release)(waker.data) }.resume("failed to release Tokio waker");
}

unsafe extern "C" fn poll_dump_empty(_: *mut c_void, _: *const Waker) -> OperationPoll {
    OperationPoll {
        state: crate::Poll::Ready,
        call: CallResult::error("Tokio runtime dump is unavailable"),
    }
}

unsafe extern "C" fn release_dump_empty(_: *mut c_void) -> CallResult {
    CallResult::ok()
}

unsafe extern "C" fn enter_empty(
    _: *mut c_void,
    state: *mut ExecutionState,
    call: GuestCall,
) -> CallResult {
    unsafe { with_execution_state(state, || call.invoke()) }
}

unsafe extern "C" fn detach_empty(_: *mut c_void) -> CallResult {
    CallResult::ok()
}

fn callback(call: impl FnOnce()) -> CallResult {
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(()) => CallResult::ok(),
        Err(payload) => CallResult::panicked(&*payload),
    }
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
