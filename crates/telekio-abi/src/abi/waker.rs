use std::{
    ffi::c_void,
    mem::{ManuallyDrop, MaybeUninit},
    ops::Deref,
    panic::{AssertUnwindSafe, catch_unwind},
    task::{RawWaker, RawWakerVTable, Waker as RustWaker},
};

use crate::CallResult;

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Waker {
    data: *const c_void,
    vtable: *const c_void,
    clone: unsafe extern "C" fn(*const Waker, *mut Waker) -> CallResult,
    wake: unsafe extern "C" fn(*const Waker) -> CallResult,
    wake_by_ref: unsafe extern "C" fn(*const Waker) -> CallResult,
    release: unsafe extern "C" fn(*const Waker) -> CallResult,
}

unsafe impl Send for Waker {}
unsafe impl Sync for Waker {}

impl Waker {
    /// # Safety
    ///
    /// `waker` must outlive every use of the returned borrowed descriptor.
    #[doc(hidden)]
    pub unsafe fn from_ref(waker: &RustWaker) -> Self {
        if std::ptr::eq(waker.vtable(), &RAW_WAKER_VTABLE) {
            return unsafe { *waker.data().cast::<Self>() };
        }
        Self {
            data: waker.data().cast(),
            vtable: (waker.vtable() as *const RawWakerVTable).cast(),
            clone,
            wake,
            wake_by_ref,
            release,
        }
    }

    /// # Safety
    ///
    /// This descriptor must contain valid waker callbacks and state.
    #[doc(hidden)]
    pub unsafe fn clone_rust_waker(&self) -> RustWaker {
        unsafe { self.borrow() }.clone()
    }

    /// # Safety
    ///
    /// The descriptor and originating waker must remain valid throughout the borrow.
    #[doc(hidden)]
    pub unsafe fn borrow(&self) -> impl Deref<Target = RustWaker> + '_ {
        ManuallyDrop::new(unsafe {
            RustWaker::new((self as *const Self).cast(), &RAW_WAKER_VTABLE)
        })
    }

    // The originating artifact alone interprets its Rust vtable.
    unsafe fn native(&self) -> RustWaker {
        unsafe { RustWaker::new(self.data.cast(), &*self.vtable.cast()) }
    }
}

unsafe extern "C" fn clone(data: *const Waker, output: *mut Waker) -> CallResult {
    callback(|| {
        let waker = ManuallyDrop::new(unsafe { (*data).native() });
        let cloned = ManuallyDrop::new(RustWaker::clone(&waker));
        let mut descriptor = unsafe { *data };
        descriptor.data = cloned.data().cast();
        descriptor.vtable = (cloned.vtable() as *const RawWakerVTable).cast();
        unsafe { output.write(descriptor) };
    })
}

unsafe extern "C" fn wake(data: *const Waker) -> CallResult {
    callback(|| unsafe { (*data).native() }.wake())
}

unsafe extern "C" fn wake_by_ref(data: *const Waker) -> CallResult {
    callback(|| ManuallyDrop::new(unsafe { (*data).native() }).wake_by_ref())
}

unsafe extern "C" fn release(data: *const Waker) -> CallResult {
    callback(|| drop(unsafe { (*data).native() }))
}

static RAW_WAKER_VTABLE: RawWakerVTable =
    RawWakerVTable::new(raw_clone, raw_wake, raw_wake_by_ref, raw_drop);

unsafe fn raw_clone(data: *const ()) -> RawWaker {
    let waker = unsafe { &*data.cast::<Waker>() };
    let mut cloned = MaybeUninit::uninit();
    unsafe { (waker.clone)(waker, cloned.as_mut_ptr()) }.resume("failed to clone Tokio waker");
    let cloned = unsafe { cloned.assume_init() };
    RawWaker::new(Box::into_raw(Box::new(cloned)).cast(), &RAW_WAKER_VTABLE)
}

unsafe fn raw_wake(data: *const ()) {
    let waker = unsafe { Box::from_raw(data.cast_mut().cast::<Waker>()) };
    unsafe { (waker.wake)(&*waker) }.resume("failed to wake Tokio task");
}

unsafe fn raw_wake_by_ref(data: *const ()) {
    let waker = unsafe { &*data.cast::<Waker>() };
    unsafe { (waker.wake_by_ref)(waker) }.resume("failed to wake Tokio task");
}

unsafe fn raw_drop(data: *const ()) {
    let waker = unsafe { Box::from_raw(data.cast_mut().cast::<Waker>()) };
    unsafe { (waker.release)(&*waker) }.resume("failed to release Tokio waker");
}

fn callback(call: impl FnOnce()) -> CallResult {
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(()) => CallResult::ok(),
        Err(payload) => CallResult::panicked(&*payload),
    }
}
