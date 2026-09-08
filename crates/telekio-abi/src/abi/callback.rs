use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

use crate::{CallResult, OwnedBytes, Status};

#[repr(C)]
pub(crate) struct CallbackOwner {
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
pub struct StringCallback {
    owner: CallbackOwner,
    call: unsafe extern "C" fn(*const c_void) -> CallResult,
}

impl CallbackOwner {
    pub(crate) fn data(&self) -> *const c_void {
        self.data
    }

    pub(crate) fn none() -> Self {
        Self {
            data: std::ptr::null(),
            release: release_none,
        }
    }

    pub(crate) fn from_arc<T: ?Sized>(callback: Arc<T>) -> Self {
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
