mod blocking;
mod builder;
mod context;
mod dump;
mod handle;
mod location;
mod metrics;
mod task;
mod task_hooks;

pub use blocking::{Blocking, BlockingTask};
pub use builder::{BuildResult, RuntimeConfig};
pub use context::{ExecutionState, RuntimeContext, execution_state, with_execution_state};
pub use dump::{DumpOperation, DumpResult};
pub use handle::{Handle, NameResult, RawHandle};
pub use metrics::{HistogramConfig, Metric, MetricResult, WorkerCallback};
pub use task::{SourceLocation, Task, TaskIdResult};
pub use task_hooks::{TaskCallback, TaskEvent};

use std::{
    any::Any,
    ffi::c_void,
    fmt,
    future::Future as RustFuture,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    pin::Pin,
    task::{Context, Poll as RustPoll},
};

use crate::{CallResult, Poll, Status, Waker};

#[repr(C)]
pub struct RawRuntime {
    owner: *mut c_void,
    handle: RawHandle,
}

pub struct Runtime {
    raw: RawRuntime,
}

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

unsafe impl Send for Runtime {}
unsafe impl Sync for Runtime {}

#[repr(C)]
pub struct Future {
    data: *mut c_void,
    poll: unsafe extern "C" fn(*mut c_void, *mut ExecutionState, *const Waker) -> Poll,
}

struct FutureState<F: RustFuture> {
    future: Option<Pin<Box<F>>>,
    result: Option<Result<F::Output, Box<dyn Any + Send>>>,
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
