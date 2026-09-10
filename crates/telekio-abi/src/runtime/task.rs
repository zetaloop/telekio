use std::{ffi::c_void, panic::Location, slice};

use crate::{CallResult, ExecutionState, Handle, Poll, Waker};

use crate::abi::Resource;

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

#[repr(C)]
pub struct TaskIdResult {
    pub call: CallResult,
    pub value: u64,
}

#[repr(C)]
pub struct Task {
    resource: Resource,
    poll: unsafe extern "C" fn(*mut c_void, *mut ExecutionState, *const Waker) -> Poll,
    cancel: unsafe extern "C" fn(*mut c_void, *mut ExecutionState) -> CallResult,
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
        poll: unsafe extern "C" fn(*mut c_void, *mut ExecutionState, *const Waker) -> Poll,
        cancel: unsafe extern "C" fn(*mut c_void, *mut ExecutionState) -> CallResult,
        release: unsafe extern "C" fn(*mut c_void) -> CallResult,
    ) -> Self {
        Self {
            resource: unsafe { Resource::from_raw(data, release) },
            poll,
            cancel,
        }
    }

    #[doc(hidden)]
    pub fn poll(&mut self, state: &mut ExecutionState, waker: &Waker) -> Poll {
        unsafe { (self.poll)(self.resource.data(), state, waker) }
    }

    /// # Safety
    ///
    /// The task must not have been cancelled or completed already.
    #[doc(hidden)]
    pub unsafe fn cancel(&mut self, state: *mut ExecutionState) {
        unsafe { (self.cancel)(self.resource.data(), state) }.resume("failed to cancel Tokio task");
    }
}

impl Handle {
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
    pub fn task_panicked(&self) -> CallResult {
        unsafe { ((*self.raw.api).task_panicked)(self.raw.context) }
    }
}
