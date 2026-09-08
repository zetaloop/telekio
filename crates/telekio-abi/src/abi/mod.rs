mod bytes;
pub(crate) mod callback;
mod waker;

pub use bytes::{Bytes, OwnedBytes};
pub use callback::{Callback, StringCallback};
pub use waker::Waker;

use std::{any::Any, ffi::c_void, panic::resume_unwind};

use crate::{
    Blocking, BlockingTask, BuildResult, ClockResult, DumpResult, DurationParts, Future,
    InstantOffset, IoDriverResult, IoInterest, IoResource, IoResult, Metric, MetricResult,
    NameResult, RuntimeConfig, Shutdown, SignalRequest, SignalResult, SourceLocation, Task,
    TaskIdResult, TimerResult, WorkerCallback,
};

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

#[repr(C)]
pub struct BoolResult {
    pub call: CallResult,
    pub value: bool,
}

#[repr(C)]
pub(crate) struct Resource {
    data: *mut c_void,
    release: unsafe extern "C" fn(*mut c_void) -> CallResult,
}

unsafe impl Send for Resource {}
unsafe impl Sync for Resource {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Poll {
    Pending,
    Ready,
    Panicked,
}

#[repr(C)]
pub struct OperationPoll {
    pub state: Poll,
    pub call: CallResult,
}

impl Resource {
    pub(crate) const fn empty() -> Self {
        Self {
            data: std::ptr::null_mut(),
            release: release_resource_empty,
        }
    }

    pub(crate) const unsafe fn from_raw(
        data: *mut c_void,
        release: unsafe extern "C" fn(*mut c_void) -> CallResult,
    ) -> Self {
        Self { data, release }
    }

    pub(crate) const fn data(&self) -> *mut c_void {
        self.data
    }

    pub(crate) fn close(&mut self) {
        if !self.data.is_null() {
            drop(std::mem::replace(self, Self::empty()));
        }
    }
}

impl Drop for Resource {
    fn drop(&mut self) {
        unsafe { (self.release)(self.data) }.resume("failed to release Tokio resource");
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

unsafe extern "C" fn release_resource_empty(_: *mut c_void) -> CallResult {
    CallResult::ok()
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
