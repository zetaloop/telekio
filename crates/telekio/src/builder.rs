use std::{
    any::Any,
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    slice, str,
    sync::Arc,
};

use crate::{CallResult, OwnedBytes, RawRuntime, Runtime, Status};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Flavor {
    CurrentThread,
    MultiThread,
    Local,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Shutdown {
    Wait,
    Background,
    Timeout,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Bytes {
    pub data: *const u8,
    pub len: usize,
}

unsafe impl Send for Bytes {}
unsafe impl Sync for Bytes {}

#[repr(C)]
pub struct Callback {
    pub data: *const c_void,
    pub call: unsafe extern "C" fn(*const c_void) -> CallResult,
    pub release: unsafe extern "C" fn(*const c_void),
}

unsafe impl Send for Callback {}
unsafe impl Sync for Callback {}

#[repr(C)]
pub struct StringCallback {
    pub data: *const c_void,
    pub call: unsafe extern "C" fn(*const c_void) -> CallResult,
    pub release: unsafe extern "C" fn(*const c_void),
}

unsafe impl Send for StringCallback {}
unsafe impl Sync for StringCallback {}

#[repr(C)]
pub struct RuntimeConfig {
    pub flavor: Flavor,
    pub enable_io: u8,
    pub enable_time: u8,
    pub start_paused: u8,
    pub worker_threads: usize,
    pub max_blocking_threads: usize,
    pub thread_name: StringCallback,
    pub thread_stack_size: usize,
    pub has_thread_stack_size: u8,
    pub after_start: Callback,
    pub before_stop: Callback,
    pub before_park: Callback,
    pub after_unpark: Callback,
    pub keep_alive_secs: u64,
    pub keep_alive_nanos: u32,
    pub has_keep_alive: u8,
    pub global_queue_interval: u32,
    pub event_interval: u32,
    pub max_io_events_per_tick: usize,
    pub name: Bytes,
}

#[repr(C)]
pub struct BuildResult {
    pub call: CallResult,
    pub runtime: RawRuntime,
    pub workers: usize,
}

impl BuildResult {
    #[doc(hidden)]
    pub fn into_runtime(self) -> std::io::Result<(Runtime, usize)> {
        self.call.into_io_result()?;
        Ok((unsafe { Runtime::from_abi(self.runtime) }, self.workers))
    }
}

impl Bytes {
    pub fn borrow(value: Option<&str>) -> Self {
        value.map_or(
            Self {
                data: std::ptr::null(),
                len: 0,
            },
            |value| Self {
                data: value.as_ptr(),
                len: value.len(),
            },
        )
    }

    /// # Safety
    ///
    /// The bytes must remain readable and contain UTF-8 for the returned borrow.
    pub unsafe fn as_str(&self) -> &str {
        if self.len == 0 {
            return "";
        }
        unsafe { str::from_utf8_unchecked(slice::from_raw_parts(self.data, self.len)) }
    }
}

impl Callback {
    pub fn none() -> Self {
        Self {
            data: std::ptr::null(),
            call: call_none,
            release: release_none,
        }
    }

    pub fn from_arc(callback: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            data: Box::into_raw(Box::new(callback)).cast(),
            call: call_callback,
            release: release_callback,
        }
    }

    pub fn is_some(&self) -> bool {
        !self.data.is_null()
    }
}

impl StringCallback {
    pub fn from_arc(callback: Arc<dyn Fn() -> String + Send + Sync>) -> Self {
        Self {
            data: Box::into_raw(Box::new(callback)).cast(),
            call: call_string_callback,
            release: release_string_callback,
        }
    }
}

unsafe extern "C" fn call_none(_: *const c_void) -> CallResult {
    call_ok(OwnedBytes::empty())
}

unsafe extern "C" fn release_none(_: *const c_void) {}

unsafe extern "C" fn call_callback(data: *const c_void) -> CallResult {
    let callback = unsafe { &*data.cast::<Arc<dyn Fn() + Send + Sync>>() };
    match catch_unwind(AssertUnwindSafe(|| callback())) {
        Ok(()) => call_ok(OwnedBytes::empty()),
        Err(payload) => call_panicked(&*payload),
    }
}

unsafe extern "C" fn release_callback(data: *const c_void) {
    drop(unsafe { Box::from_raw(data.cast_mut().cast::<Arc<dyn Fn() + Send + Sync>>()) });
}

unsafe extern "C" fn call_string_callback(data: *const c_void) -> CallResult {
    let callback = unsafe { &*data.cast::<Arc<dyn Fn() -> String + Send + Sync>>() };
    match catch_unwind(AssertUnwindSafe(|| callback())) {
        Ok(value) => call_ok(OwnedBytes::from_string(value)),
        Err(payload) => call_panicked(&*payload),
    }
}

unsafe extern "C" fn release_string_callback(data: *const c_void) {
    drop(unsafe {
        Box::from_raw(
            data.cast_mut()
                .cast::<Arc<dyn Fn() -> String + Send + Sync>>(),
        )
    });
}

fn call_ok(payload: OwnedBytes) -> CallResult {
    CallResult {
        status: Status::Ok,
        payload,
    }
}

fn call_panicked(payload: &(dyn Any + Send)) -> CallResult {
    CallResult {
        status: Status::Panicked,
        payload: OwnedBytes::from_string(panic_message(payload)),
    }
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
