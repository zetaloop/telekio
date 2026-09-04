use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Mutex,
};

use crate::{CallResult, ExecutionState, Handle, RawHandle, Status, with_execution_state};

static ATTACHED: Mutex<Option<Handle>> = Mutex::new(None);

#[repr(C)]
pub struct RawAttachment {
    data: *mut c_void,
    enter: unsafe extern "C" fn(*mut c_void, *mut ExecutionState, GuestCall) -> CallResult,
    detach: unsafe extern "C" fn(*mut c_void) -> CallResult,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct GuestCall {
    data: *mut c_void,
    call: unsafe extern "C" fn(*mut c_void) -> CallResult,
}

#[repr(C)]
pub struct AttachResult {
    pub call: CallResult,
    pub attachment: RawAttachment,
}

unsafe impl Send for RawAttachment {}
unsafe impl Sync for RawAttachment {}

impl RawAttachment {
    pub const fn empty() -> Self {
        Self {
            data: std::ptr::null_mut(),
            enter: enter_empty,
            detach: detach_empty,
        }
    }

    /// # Safety
    ///
    /// `data` and the callbacks must describe one owned guest execution
    /// context that remains valid until `detach` is called.
    #[doc(hidden)]
    pub const unsafe fn from_raw(
        data: *mut c_void,
        enter: unsafe extern "C" fn(*mut c_void, *mut ExecutionState, GuestCall) -> CallResult,
        detach: unsafe extern "C" fn(*mut c_void) -> CallResult,
    ) -> Self {
        Self {
            data,
            enter,
            detach,
        }
    }

    /// # Safety
    ///
    /// The plugin defining these callbacks must remain loaded.
    #[doc(hidden)]
    pub unsafe fn enter(&self, state: *mut ExecutionState, call: GuestCall) -> CallResult {
        unsafe { (self.enter)(self.data, state, call) }
    }

    /// # Safety
    ///
    /// The plugin defining these callbacks must remain loaded, and its owner
    /// must have completed shutdown.
    #[doc(hidden)]
    pub unsafe fn detach(&self) -> CallResult {
        unsafe { (self.detach)(self.data) }
    }
}

impl GuestCall {
    /// # Safety
    ///
    /// `data` must remain valid for one synchronous invocation of `call`.
    #[doc(hidden)]
    pub const unsafe fn from_raw(
        data: *mut c_void,
        call: unsafe extern "C" fn(*mut c_void) -> CallResult,
    ) -> Self {
        Self { data, call }
    }

    /// # Safety
    ///
    /// The backing call state must still be valid and may be invoked once.
    #[doc(hidden)]
    pub unsafe fn invoke(self) -> CallResult {
        unsafe { (self.call)(self.data) }
    }
}

#[doc(hidden)]
pub fn install_handle(handle: Handle) -> Result<(), Handle> {
    let mut attached = ATTACHED.lock().unwrap();
    if attached.is_some() {
        Err(handle)
    } else {
        *attached = Some(handle);
        Ok(())
    }
}

#[cfg(feature = "guest")]
unsafe extern "C" {
    fn telekio_guest_context(raw: RawHandle) -> AttachResult;
}

/// # Safety
///
/// `raw` must be one owned host handle reference returned by its host API.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telekio_guest_attach(raw: RawHandle) -> AttachResult {
    #[cfg(feature = "guest")]
    {
        unsafe { telekio_guest_context(raw) }
    }
    #[cfg(not(feature = "guest"))]
    {
        drop(unsafe { Handle::from_abi(raw) });
        crate::require_package("This package");
        AttachResult {
            call: CallResult::error("Telekio guest is not enabled"),
            attachment: RawAttachment::empty(),
        }
    }
}

/// # Safety
///
/// The guest execution context must have been released, and the attached host
/// owner must have completed shutdown.
#[doc(hidden)]
pub unsafe fn detach_attached() -> CallResult {
    match catch_unwind(AssertUnwindSafe(|| {
        let handle = ATTACHED
            .lock()
            .unwrap()
            .take()
            .ok_or("Telekio runtime is not attached")?;
        let result = unsafe { ((*handle.raw.api).detach)(handle.raw.context) };
        if result.status == Status::Ok {
            std::mem::forget(handle);
        } else {
            *ATTACHED.lock().unwrap() = Some(handle);
        }
        Ok::<_, &str>(result)
    })) {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => CallResult::error(error),
        Err(payload) => CallResult::panicked(&*payload),
    }
}

#[doc(hidden)]
pub fn attached() -> Handle {
    if let Some(handle) = ATTACHED.lock().unwrap().as_ref() {
        return handle.clone();
    }
    #[cfg(feature = "guest")]
    {
        unsafe extern "C-unwind" {
            fn telekio_default_handle() -> RawHandle;
        }
        let raw = unsafe { telekio_default_handle() };
        if !raw.is_empty() {
            let handle = unsafe { Handle::from_abi(raw) };
            if install_handle(handle).is_ok() {
                return ATTACHED.lock().unwrap().as_ref().unwrap().clone();
            }
        }
    }
    panic!("Telekio runtime is not attached")
}

unsafe extern "C" fn enter_empty(
    _: *mut c_void,
    state: *mut ExecutionState,
    call: GuestCall,
) -> CallResult {
    unsafe { with_execution_state(state, || call.invoke()) }
}

unsafe extern "C" fn detach_empty(_: *mut c_void) -> CallResult {
    CallResult::ok()
}
