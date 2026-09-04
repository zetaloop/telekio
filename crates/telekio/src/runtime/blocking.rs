use std::ffi::c_void;

use crate::{CallResult, ExecutionState, Handle, SourceLocation, Status};

use crate::abi::Resource;

#[repr(C)]
pub struct BlockingTask {
    resource: Resource,
    run: unsafe extern "C" fn(*mut c_void, *mut ExecutionState) -> CallResult,
    cancel: unsafe extern "C" fn(*mut c_void, *mut ExecutionState) -> CallResult,
}

unsafe impl Send for BlockingTask {}

#[repr(C)]
pub struct Blocking {
    data: *mut c_void,
    run: unsafe extern "C" fn(*mut c_void) -> Status,
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
            resource: unsafe { Resource::from_raw(data, release) },
            run,
            cancel,
        }
    }

    /// # Safety
    ///
    /// This task must not have been run or cancelled already.
    #[doc(hidden)]
    pub unsafe fn run(&mut self, state: *mut ExecutionState) {
        unsafe { (self.run)(self.resource.data(), state) }
            .resume("failed to run Tokio blocking task");
    }

    /// # Safety
    ///
    /// This task must not have been run or cancelled already.
    #[doc(hidden)]
    pub unsafe fn cancel(&mut self, state: *mut ExecutionState) {
        unsafe { (self.cancel)(self.resource.data(), state) }
            .resume("failed to cancel Tokio blocking task");
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

impl Handle {
    #[doc(hidden)]
    pub fn spawn_blocking(
        &self,
        task: BlockingTask,
        id: u64,
        location: SourceLocation,
    ) -> CallResult {
        unsafe { ((*self.raw.api).spawn_blocking)(self.raw.context, task, id, location) }
    }

    pub fn block_in_place(&self, blocking: Blocking) -> CallResult {
        unsafe { ((*self.raw.api).block_in_place)(self.raw.context, blocking) }
    }
}
