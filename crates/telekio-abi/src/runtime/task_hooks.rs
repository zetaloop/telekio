use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, Location, catch_unwind},
    sync::Arc,
};

use crate::{CallResult, SourceLocation};

use crate::abi::callback::CallbackOwner;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TaskEvent {
    Spawn,
    PollStart,
    PollStop,
    Terminate,
}

#[repr(C)]
pub struct TaskCallback {
    owner: CallbackOwner,
    call: unsafe extern "C" fn(*const c_void, TaskEvent, u64, *const SourceLocation) -> CallResult,
}

impl TaskCallback {
    pub fn none() -> Self {
        Self {
            owner: CallbackOwner::none(),
            call: call_no_task_callback,
        }
    }

    pub fn from_arc(
        callback: Arc<dyn Fn(TaskEvent, u64, &'static Location<'static>) + Send + Sync>,
    ) -> Self {
        Self {
            owner: CallbackOwner::from_arc(callback),
            call: call_task_callback,
        }
    }

    pub fn is_some(&self) -> bool {
        !self.owner.data().is_null()
    }

    #[doc(hidden)]
    pub fn call(
        &self,
        event: TaskEvent,
        id: u64,
        location: &'static Location<'static>,
    ) -> CallResult {
        unsafe {
            (self.call)(
                self.owner.data(),
                event,
                id,
                super::location::borrow(location),
            )
        }
    }
}

unsafe extern "C" fn call_no_task_callback(
    _: *const c_void,
    _: TaskEvent,
    _: u64,
    _: *const SourceLocation,
) -> CallResult {
    CallResult::ok()
}

unsafe extern "C" fn call_task_callback(
    data: *const c_void,
    event: TaskEvent,
    id: u64,
    location: *const SourceLocation,
) -> CallResult {
    let callback = unsafe {
        &*data.cast::<Arc<dyn Fn(TaskEvent, u64, &'static Location<'static>) + Send + Sync>>()
    };
    match catch_unwind(AssertUnwindSafe(|| {
        callback(event, id, unsafe { super::location::resolve(location) })
    })) {
        Ok(()) => CallResult::ok(),
        Err(payload) => CallResult::panicked(&*payload),
    }
}
