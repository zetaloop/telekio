use std::{ffi::c_void, task::Poll as RustPoll};

use crate::{CallResult, Handle, OperationPoll, Status, Waker};

use crate::abi::Resource;

#[repr(C)]
pub struct DumpOperation {
    resource: Resource,
    poll: unsafe extern "C" fn(*mut c_void, *const Waker) -> OperationPoll,
}

#[repr(C)]
pub struct DumpResult {
    pub call: CallResult,
    pub dump: DumpOperation,
}

unsafe impl Send for DumpOperation {}

impl DumpOperation {
    pub const fn empty() -> Self {
        Self {
            resource: Resource::empty(),
            poll: poll_dump_empty,
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
            resource: unsafe { Resource::from_raw(data, release) },
            poll,
        }
    }

    #[doc(hidden)]
    pub fn poll(&mut self, waker: &Waker) -> RustPoll<Vec<u8>> {
        let result = unsafe { (self.poll)(self.resource.data(), waker) };
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

unsafe extern "C" fn poll_dump_empty(_: *mut c_void, _: *const Waker) -> OperationPoll {
    OperationPoll {
        state: crate::Poll::Ready,
        call: CallResult::error("Tokio runtime dump is unavailable"),
    }
}

impl Handle {
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
}
