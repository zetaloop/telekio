use std::{
    ffi::c_void,
    mem::MaybeUninit,
    panic::{AssertUnwindSafe, catch_unwind},
    task::{RawWaker, RawWakerVTable, Waker as RustWaker},
};

use crate::CallResult;

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

fn callback(call: impl FnOnce()) -> CallResult {
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(()) => CallResult::ok(),
        Err(payload) => CallResult::panicked(&*payload),
    }
}
