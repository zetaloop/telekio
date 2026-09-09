use std::{ffi::c_void, fmt, future::Future as RustFuture};

use crate::{CallResult, OwnedBytes, RuntimeApi, Waker};

use super::block_on;

#[derive(Clone, Copy)]
#[repr(C)]
pub struct RawHandle {
    pub(crate) context: *const c_void,
    pub(crate) api: *const RuntimeApi,
}

pub struct Handle {
    pub(crate) raw: RawHandle,
}

unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

#[repr(C)]
pub struct NameResult {
    pub call: CallResult,
    pub value: OwnedBytes,
    pub is_some: bool,
}

impl RawHandle {
    pub const fn is_empty(self) -> bool {
        self.context.is_null() || self.api.is_null()
    }

    pub const fn empty() -> Self {
        Self {
            context: std::ptr::null(),
            api: std::ptr::null(),
        }
    }

    /// # Safety
    ///
    /// `context` and `api` must form one valid host handle reference.
    pub const unsafe fn from_raw(context: *const c_void, api: *const RuntimeApi) -> Self {
        Self { context, api }
    }
}

impl Handle {
    /// # Safety
    ///
    /// `raw` must be one owned host handle reference returned by its host API.
    pub const unsafe fn from_abi(raw: RawHandle) -> Self {
        Self { raw }
    }

    pub fn into_abi(self) -> RawHandle {
        let raw = self.raw;
        std::mem::forget(self);
        raw
    }

    pub fn block_on<F: RustFuture>(&self, future: F) -> F::Output {
        block_on(future, |future| unsafe {
            ((*self.raw.api).handle_block_on)(self.raw.context, future)
        })
    }

    #[doc(hidden)]
    pub fn flavor(&self) -> crate::Flavor {
        unsafe { ((*self.raw.api).flavor)(self.raw.context) }
    }

    #[doc(hidden)]
    pub fn can_spawn_local(&self) -> bool {
        let result = unsafe { ((*self.raw.api).can_spawn_local)(self.raw.context) };
        result
            .call
            .resume("failed to read Tokio local runtime context");
        result.value
    }

    #[doc(hidden)]
    pub fn id(&self) -> u64 {
        unsafe { ((*self.raw.api).id)(self.raw.context) }
    }

    #[doc(hidden)]
    pub fn name(&self) -> Option<String> {
        let result = unsafe { ((*self.raw.api).name)(self.raw.context) };
        result.call.resume("failed to read Tokio runtime name");
        if result.is_some {
            Some(unsafe { result.value.into_string() })
        } else {
            unsafe { result.value.release() };
            None
        }
    }

    #[doc(hidden)]
    pub fn defer(&self, waker: &Waker) -> CallResult {
        unsafe { ((*self.raw.api).defer)(self.raw.context, waker) }
    }

    #[doc(hidden)]
    pub fn reap_process(&self, id: u32) -> CallResult {
        unsafe { ((*self.raw.api).reap_process)(self.raw.context, id) }
    }
}

impl Clone for Handle {
    fn clone(&self) -> Self {
        unsafe { ((*self.raw.api).retain_handle)(self.raw.context) }
            .resume("failed to retain Tokio handle");
        Self { raw: self.raw }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { ((*self.raw.api).release_handle)(self.raw.context) }
            .resume("failed to release Tokio handle");
    }
}

impl fmt::Debug for Handle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Handle")
            .field("context", &self.raw.context)
            .finish_non_exhaustive()
    }
}
