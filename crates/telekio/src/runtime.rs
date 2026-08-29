use crate::{
    BuildResult, ClockSample, DurationParts, IoInterest, IoResource, IoResult, RuntimeConfig,
    Shutdown, SignalRequest, SignalResult, TimerResult,
};

use std::{
    any::Any,
    ffi::c_void,
    fmt,
    future::Future as RustFuture,
    mem::MaybeUninit,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    pin::Pin,
    slice,
    sync::OnceLock,
    task::{Context, Poll as RustPoll, RawWaker, RawWakerVTable, Waker as RustWaker},
};

static ATTACHED: OnceLock<Handle> = OnceLock::new();

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

pub struct Handle {
    pub(crate) raw: RawHandle,
}

pub struct Runtime {
    raw: RawRuntime,
}

unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
unsafe impl Send for Runtime {}
unsafe impl Sync for Runtime {}

#[repr(C)]
pub struct RuntimeApi {
    pub runtime_block_on: unsafe extern "C" fn(*mut c_void, Future) -> CallResult,
    pub handle_block_on: unsafe extern "C" fn(*const c_void, Future) -> CallResult,
    pub retain_handle: unsafe extern "C" fn(*const c_void),
    pub release_handle: unsafe extern "C" fn(*const c_void),
    pub release_runtime: unsafe extern "C" fn(*mut c_void),
    pub spawn: unsafe extern "C" fn(*const c_void, u64, Task) -> CallResult,
    pub spawn_local: unsafe extern "C" fn(*const c_void, u64, Task) -> CallResult,
    pub spawn_blocking: unsafe extern "C" fn(*const c_void, u64, BlockingTask) -> CallResult,
    pub block_in_place: unsafe extern "C" fn(*const c_void, Blocking) -> CallResult,
    pub abort: unsafe extern "C" fn(*const c_void, u64),
    pub is_finished: unsafe extern "C" fn(*const c_void, u64) -> bool,
    pub build: unsafe extern "C" fn(*const c_void, RuntimeConfig) -> BuildResult,
    pub clock: unsafe extern "C" fn(*const c_void) -> ClockSample,
    pub pause: unsafe extern "C" fn(*const c_void) -> CallResult,
    pub resume: unsafe extern "C" fn(*const c_void) -> CallResult,
    pub advance: unsafe extern "C" fn(*const c_void, DurationParts) -> CallResult,
    pub timer: unsafe extern "C" fn(*const c_void, DurationParts) -> TimerResult,
    pub register_io: unsafe extern "C" fn(*const c_void, IoResource, IoInterest) -> IoResult,
    pub signal: unsafe extern "C" fn(*const c_void, SignalRequest) -> SignalResult,
    pub reap_process: unsafe extern "C" fn(*const c_void, u32) -> CallResult,
    pub shutdown: unsafe extern "C" fn(*mut c_void, Shutdown, u64, u32) -> CallResult,
    pub defer: unsafe extern "C" fn(*const c_void, *const Waker) -> CallResult,
    pub metric: unsafe extern "C" fn(*const c_void, Metric, usize) -> MetricResult,
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
    WorkerTotalBusyDuration,
    WorkerParkCount,
    WorkerParkUnparkCount,
}

#[repr(C)]
pub struct MetricResult {
    pub call: CallResult,
    pub value: u64,
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
    poll: unsafe extern "C" fn(*mut c_void, *const Waker) -> Poll,
}

#[repr(C)]
pub struct Task {
    data: *mut c_void,
    poll: unsafe extern "C" fn(*mut c_void, *const Waker) -> Poll,
    cancel: unsafe extern "C" fn(*mut c_void),
    release: unsafe extern "C" fn(*mut c_void),
}

#[repr(C)]
pub struct BlockingTask {
    data: *mut c_void,
    run: unsafe extern "C" fn(*mut c_void),
    cancel: unsafe extern "C" fn(*mut c_void),
    release: unsafe extern "C" fn(*mut c_void),
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
    clone: unsafe extern "C" fn(*const c_void, *mut Waker),
    wake: unsafe extern "C" fn(*const c_void),
    wake_by_ref: unsafe extern "C" fn(*const c_void),
    release: unsafe extern "C" fn(*const c_void),
}

unsafe impl Send for Waker {}
unsafe impl Sync for Waker {}

struct FutureState<F: RustFuture> {
    future: Option<Pin<Box<F>>>,
    result: Option<Result<F::Output, Box<dyn Any + Send>>>,
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
        poll: unsafe extern "C" fn(*mut c_void, *const Waker) -> Poll,
    ) -> Self {
        Self { data, poll }
    }

    #[doc(hidden)]
    pub fn poll(&mut self, waker: &Waker) -> Poll {
        unsafe { (self.poll)(self.data, waker) }
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
        poll: unsafe extern "C" fn(*mut c_void, *const Waker) -> Poll,
        cancel: unsafe extern "C" fn(*mut c_void),
        release: unsafe extern "C" fn(*mut c_void),
    ) -> Self {
        Self {
            data,
            poll,
            cancel,
            release,
        }
    }

    #[doc(hidden)]
    pub fn poll(&mut self, waker: &Waker) -> Poll {
        unsafe { (self.poll)(self.data, waker) }
    }

    /// # Safety
    ///
    /// The task must not have been cancelled or completed already.
    #[doc(hidden)]
    pub unsafe fn cancel(&mut self) {
        unsafe { (self.cancel)(self.data) };
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        unsafe { (self.release)(self.data) };
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
        run: unsafe extern "C" fn(*mut c_void),
        cancel: unsafe extern "C" fn(*mut c_void),
        release: unsafe extern "C" fn(*mut c_void),
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
    pub unsafe fn run(&mut self) {
        unsafe { (self.run)(self.data) };
    }

    /// # Safety
    ///
    /// This task must not have been run or cancelled already.
    #[doc(hidden)]
    pub unsafe fn cancel(&mut self) {
        unsafe { (self.cancel)(self.data) };
    }
}

impl Drop for BlockingTask {
    fn drop(&mut self) {
        unsafe { (self.release)(self.data) };
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

pub fn attach(handle: Handle) -> Result<(), Handle> {
    ATTACHED.set(handle)
}

#[cfg(feature = "guest")]
/// # Safety
///
/// `raw` must be one owned host handle reference returned by its host API.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telekio_guest_attach(raw: RawHandle) -> bool {
    attach(unsafe { Handle::from_abi(raw) }).is_ok()
}

#[doc(hidden)]
pub fn attached() -> Handle {
    if let Some(handle) = ATTACHED.get() {
        return handle.clone();
    }
    #[cfg(feature = "guest")]
    {
        unsafe extern "C" {
            fn telekio_test_handle() -> RawHandle;
        }
        let raw = unsafe { telekio_test_handle() };
        if !raw.is_empty() {
            let handle = unsafe { Handle::from_abi(raw) };
            if attach(handle).is_ok() {
                return ATTACHED.get().unwrap().clone();
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
    pub fn reap_process(&self, id: u32) -> CallResult {
        unsafe { ((*self.raw.api).reap_process)(self.raw.context, id) }
    }

    pub fn spawn(&self, id: u64, task: Task) -> CallResult {
        unsafe { ((*self.raw.api).spawn)(self.raw.context, id, task) }
    }

    #[doc(hidden)]
    pub fn spawn_local(&self, id: u64, task: Task) -> CallResult {
        unsafe { ((*self.raw.api).spawn_local)(self.raw.context, id, task) }
    }

    #[doc(hidden)]
    pub fn spawn_blocking(&self, id: u64, task: BlockingTask) -> CallResult {
        unsafe { ((*self.raw.api).spawn_blocking)(self.raw.context, id, task) }
    }

    #[doc(hidden)]
    pub fn defer(&self, waker: &Waker) -> CallResult {
        unsafe { ((*self.raw.api).defer)(self.raw.context, waker) }
    }

    #[doc(hidden)]
    #[track_caller]
    pub fn metric(&self, metric: Metric, worker: usize) -> u64 {
        let result = unsafe { ((*self.raw.api).metric)(self.raw.context, metric, worker) };
        result.call.into_io_result().unwrap();
        result.value
    }

    pub fn block_in_place(&self, blocking: Blocking) -> CallResult {
        unsafe { ((*self.raw.api).block_in_place)(self.raw.context, blocking) }
    }

    #[doc(hidden)]
    pub fn abort(&self, id: u64) {
        unsafe { ((*self.raw.api).abort)(self.raw.context, id) };
    }

    #[doc(hidden)]
    pub fn is_finished(&self, id: u64) -> bool {
        unsafe { ((*self.raw.api).is_finished)(self.raw.context, id) }
    }

    #[doc(hidden)]
    pub fn build(&self, config: RuntimeConfig) -> BuildResult {
        unsafe { ((*self.raw.api).build)(self.raw.context, config) }
    }
}

impl Clone for Handle {
    fn clone(&self) -> Self {
        unsafe { ((*self.raw.api).retain_handle)(self.raw.context) };
        Self { raw: self.raw }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { ((*self.raw.api).release_handle)(self.raw.context) };
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
        unsafe { ((*self.raw.handle.api).retain_handle)(self.raw.handle.context) };
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
        unsafe {
            ((*self.raw.handle.api).release_runtime)(self.raw.owner);
            ((*self.raw.handle.api).release_handle)(self.raw.handle.context);
        }
    }
}

impl CallResult {
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
        let bytes = value.into_bytes().into_boxed_slice();
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
        let bytes = if self.len == 0 {
            Vec::new()
        } else {
            unsafe { slice::from_raw_parts(self.data, self.len) }.to_vec()
        };
        unsafe { self.release() };
        String::from_utf8(bytes).unwrap_or_else(|_| "host Tokio runtime panicked".to_owned())
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

unsafe extern "C" fn poll_future<F: RustFuture>(data: *mut c_void, waker: *const Waker) -> Poll {
    let state = unsafe { &mut *data.cast::<FutureState<F>>() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let waker = unsafe { (*waker).clone_rust_waker() };
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
        unsafe { (self.clone)(self.data, owned.as_mut_ptr()) };
        let owned = unsafe { owned.assume_init() };
        let raw = RawWaker::new(Box::into_raw(Box::new(owned)).cast(), &RAW_WAKER_VTABLE);
        unsafe { RustWaker::from_raw(raw) }
    }
}

unsafe extern "C" fn clone_borrowed(data: *const c_void, output: *mut Waker) {
    let waker = unsafe { &*data.cast::<RustWaker>() };
    unsafe { output.write(owned_waker(waker.clone())) };
}

unsafe extern "C" fn wake_borrowed(data: *const c_void) {
    unsafe { &*data.cast::<RustWaker>() }.wake_by_ref();
}

unsafe extern "C" fn release_borrowed(_: *const c_void) {}

fn owned_waker(waker: RustWaker) -> Waker {
    Waker {
        data: Box::into_raw(Box::new(waker)).cast(),
        clone: clone_owned,
        wake: wake_owned,
        wake_by_ref: wake_by_ref_owned,
        release: release_owned,
    }
}

unsafe extern "C" fn clone_owned(data: *const c_void, output: *mut Waker) {
    let waker = unsafe { &*data.cast::<RustWaker>() };
    unsafe { output.write(owned_waker(waker.clone())) };
}

unsafe extern "C" fn wake_owned(data: *const c_void) {
    unsafe { Box::from_raw(data.cast_mut().cast::<RustWaker>()) }.wake();
}

unsafe extern "C" fn wake_by_ref_owned(data: *const c_void) {
    unsafe { &*data.cast::<RustWaker>() }.wake_by_ref();
}

unsafe extern "C" fn release_owned(data: *const c_void) {
    drop(unsafe { Box::from_raw(data.cast_mut().cast::<RustWaker>()) });
}

static RAW_WAKER_VTABLE: RawWakerVTable =
    RawWakerVTable::new(raw_clone, raw_wake, raw_wake_by_ref, raw_drop);

unsafe fn raw_clone(data: *const ()) -> RawWaker {
    let waker = unsafe { &*data.cast::<Waker>() };
    let mut cloned = MaybeUninit::uninit();
    unsafe { (waker.clone)(waker.data, cloned.as_mut_ptr()) };
    let cloned = unsafe { cloned.assume_init() };
    RawWaker::new(Box::into_raw(Box::new(cloned)).cast(), &RAW_WAKER_VTABLE)
}

unsafe fn raw_wake(data: *const ()) {
    let waker = unsafe { Box::from_raw(data.cast_mut().cast::<Waker>()) };
    unsafe { (waker.wake)(waker.data) };
}

unsafe fn raw_wake_by_ref(data: *const ()) {
    let waker = unsafe { &*data.cast::<Waker>() };
    unsafe { (waker.wake_by_ref)(waker.data) };
}

unsafe fn raw_drop(data: *const ()) {
    let waker = unsafe { Box::from_raw(data.cast_mut().cast::<Waker>()) };
    unsafe { (waker.release)(waker.data) };
}
