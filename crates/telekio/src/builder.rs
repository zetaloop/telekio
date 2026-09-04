use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    slice, str,
    sync::Arc,
};

use crate::{CallResult, OwnedBytes, RawRuntime, Runtime, Status};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Flavor {
    CurrentThread,
    MultiThread,
    Local,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Shutdown {
    Wait,
    Background,
    Timeout,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TaskEvent {
    Spawn,
    PollStart,
    PollStop,
    Terminate,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Bytes {
    data: *const u8,
    len: usize,
}

unsafe impl Send for Bytes {}
unsafe impl Sync for Bytes {}

#[repr(C)]
struct CallbackOwner {
    data: *const c_void,
    release: unsafe extern "C" fn(*const c_void) -> CallResult,
}

unsafe impl Send for CallbackOwner {}
unsafe impl Sync for CallbackOwner {}

#[repr(C)]
pub struct Callback {
    owner: CallbackOwner,
    call: unsafe extern "C" fn(*const c_void) -> CallResult,
}

#[repr(C)]
pub struct WorkerCallback {
    owner: CallbackOwner,
    call: unsafe extern "C" fn(*const c_void, usize) -> CallResult,
}

#[repr(C)]
pub struct TaskCallback {
    owner: CallbackOwner,
    call: unsafe extern "C" fn(*const c_void, TaskEvent, u64) -> CallResult,
}

#[repr(C)]
pub struct StringCallback {
    owner: CallbackOwner,
    call: unsafe extern "C" fn(*const c_void) -> CallResult,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct HistogramConfig {
    pub kind: u8,
    pub a: u64,
    pub b: u64,
    pub c: u64,
}

#[repr(C)]
pub struct RuntimeConfig {
    pub flavor: Flavor,
    pub enable_io: u8,
    pub enable_time: u8,
    pub start_paused: u8,
    pub worker_threads: usize,
    pub max_blocking_threads: usize,
    pub thread_name: StringCallback,
    pub thread_stack_size: usize,
    pub has_thread_stack_size: u8,
    pub after_start: Callback,
    pub before_stop: Callback,
    pub before_park: Callback,
    pub after_unpark: Callback,
    pub task_callback: TaskCallback,
    pub keep_alive_secs: u64,
    pub keep_alive_nanos: u32,
    pub has_keep_alive: u8,
    pub global_queue_interval: u32,
    pub event_interval: u32,
    pub max_io_events_per_tick: usize,
    pub rng_one: u32,
    pub rng_two: u32,
    pub name: Bytes,
    pub disable_lifo_slot: u8,
    pub eager_driver_handoff: u8,
    pub alternative_timer: u8,
    pub unhandled_panic: u8,
    pub poll_histogram: HistogramConfig,
    pub schedule_histogram: HistogramConfig,
}

#[repr(C)]
pub struct BuildResult {
    call: CallResult,
    runtime: RawRuntime,
    workers: usize,
}

impl HistogramConfig {
    #[doc(hidden)]
    pub const fn disabled() -> Self {
        Self {
            kind: 0,
            a: 0,
            b: 0,
            c: 0,
        }
    }
}

impl BuildResult {
    #[doc(hidden)]
    pub fn success(runtime: RawRuntime, workers: usize) -> Self {
        assert!(!runtime.is_empty(), "host returned an empty Tokio runtime");
        Self {
            call: CallResult {
                status: Status::Ok,
                payload: OwnedBytes::empty(),
            },
            runtime,
            workers,
        }
    }

    #[doc(hidden)]
    pub fn error(call: CallResult) -> Self {
        Self {
            call,
            runtime: RawRuntime::empty(),
            workers: 0,
        }
    }

    /// # Safety
    ///
    /// A runtime built with [`Flavor::Local`] must remain on its originating
    /// thread until it has shut down.
    #[doc(hidden)]
    pub unsafe fn into_runtime(self) -> std::io::Result<(Runtime, usize)> {
        self.call.into_io_result()?;
        Ok((unsafe { Runtime::from_abi(self.runtime) }, self.workers))
    }
}

impl Bytes {
    /// # Safety
    ///
    /// The borrowed string must remain valid until the host build call returns.
    #[doc(hidden)]
    pub unsafe fn borrow(value: Option<&str>) -> Self {
        value.map_or(
            Self {
                data: std::ptr::null(),
                len: 0,
            },
            |value| Self {
                data: value.as_ptr(),
                len: value.len(),
            },
        )
    }

    /// # Safety
    ///
    /// The bytes must remain readable and contain UTF-8 for the returned borrow.
    pub unsafe fn as_str(&self) -> &str {
        if self.len == 0 {
            return "";
        }
        unsafe { str::from_utf8_unchecked(slice::from_raw_parts(self.data, self.len)) }
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl CallbackOwner {
    fn none() -> Self {
        Self {
            data: std::ptr::null(),
            release: release_none,
        }
    }

    fn from_arc<T: ?Sized>(callback: Arc<T>) -> Self {
        Self {
            data: Box::into_raw(Box::new(callback)).cast(),
            release: release_arc::<T>,
        }
    }
}

impl Drop for CallbackOwner {
    fn drop(&mut self) {
        unsafe { (self.release)(self.data) }.resume("failed to release Tokio callback");
    }
}

impl Callback {
    pub fn none() -> Self {
        Self {
            owner: CallbackOwner::none(),
            call: call_none,
        }
    }

    pub fn from_arc(callback: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            owner: CallbackOwner::from_arc(callback),
            call: call_callback,
        }
    }

    pub fn is_some(&self) -> bool {
        !self.owner.data.is_null()
    }

    #[doc(hidden)]
    pub fn call(&self) -> CallResult {
        unsafe { (self.call)(self.owner.data) }
    }
}

impl WorkerCallback {
    pub fn from_arc(callback: Arc<dyn Fn(usize) + Send + Sync>) -> Self {
        Self {
            owner: CallbackOwner::from_arc(callback),
            call: call_worker_callback,
        }
    }

    #[doc(hidden)]
    pub fn call(&self, worker: usize) -> CallResult {
        unsafe { (self.call)(self.owner.data, worker) }
    }
}

impl TaskCallback {
    pub fn none() -> Self {
        Self {
            owner: CallbackOwner::none(),
            call: call_no_task_callback,
        }
    }

    pub fn from_arc(callback: Arc<dyn Fn(TaskEvent, u64) + Send + Sync>) -> Self {
        Self {
            owner: CallbackOwner::from_arc(callback),
            call: call_task_callback,
        }
    }

    pub fn is_some(&self) -> bool {
        !self.owner.data.is_null()
    }

    #[doc(hidden)]
    pub fn call(&self, event: TaskEvent, id: u64) -> CallResult {
        unsafe { (self.call)(self.owner.data, event, id) }
    }
}

impl StringCallback {
    pub fn from_arc(callback: Arc<dyn Fn() -> String + Send + Sync>) -> Self {
        Self {
            owner: CallbackOwner::from_arc(callback),
            call: call_string_callback,
        }
    }

    #[doc(hidden)]
    pub fn call(&self) -> CallResult {
        unsafe { (self.call)(self.owner.data) }
    }
}

unsafe extern "C" fn call_none(_: *const c_void) -> CallResult {
    call_ok(OwnedBytes::empty())
}

unsafe extern "C" fn release_none(_: *const c_void) -> CallResult {
    CallResult::ok()
}

unsafe extern "C" fn call_callback(data: *const c_void) -> CallResult {
    let callback = unsafe { &*data.cast::<Arc<dyn Fn() + Send + Sync>>() };
    match catch_unwind(AssertUnwindSafe(|| callback())) {
        Ok(()) => call_ok(OwnedBytes::empty()),
        Err(payload) => CallResult::panicked(&*payload),
    }
}

unsafe extern "C" fn release_arc<T: ?Sized>(data: *const c_void) -> CallResult {
    match catch_unwind(AssertUnwindSafe(|| {
        drop(unsafe { Box::from_raw(data.cast_mut().cast::<Arc<T>>()) });
    })) {
        Ok(()) => CallResult::ok(),
        Err(payload) => CallResult::panicked(&*payload),
    }
}

unsafe extern "C" fn call_no_task_callback(_: *const c_void, _: TaskEvent, _: u64) -> CallResult {
    CallResult::ok()
}

unsafe extern "C" fn call_task_callback(
    data: *const c_void,
    event: TaskEvent,
    id: u64,
) -> CallResult {
    let callback = unsafe { &*data.cast::<Arc<dyn Fn(TaskEvent, u64) + Send + Sync>>() };
    match catch_unwind(AssertUnwindSafe(|| callback(event, id))) {
        Ok(()) => CallResult::ok(),
        Err(payload) => CallResult::panicked(&*payload),
    }
}

unsafe extern "C" fn call_worker_callback(data: *const c_void, worker: usize) -> CallResult {
    let callback = unsafe { &*data.cast::<Arc<dyn Fn(usize) + Send + Sync>>() };
    match catch_unwind(AssertUnwindSafe(|| callback(worker))) {
        Ok(()) => CallResult::ok(),
        Err(payload) => CallResult::panicked(&*payload),
    }
}

unsafe extern "C" fn call_string_callback(data: *const c_void) -> CallResult {
    let callback = unsafe { &*data.cast::<Arc<dyn Fn() -> String + Send + Sync>>() };
    match catch_unwind(AssertUnwindSafe(|| callback())) {
        Ok(value) => call_ok(OwnedBytes::from_string(value)),
        Err(payload) => CallResult::panicked(&*payload),
    }
}

fn call_ok(payload: OwnedBytes) -> CallResult {
    CallResult {
        status: Status::Ok,
        payload,
    }
}
