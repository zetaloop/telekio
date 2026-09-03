use super::*;
use crate::runtime::io::telekio::Source;
use std::panic::{AssertUnwindSafe, catch_unwind};

impl<T: AioSource> Source for MioSource<T> {
    fn telekio_resource(&mut self) -> ::telekio::IoResource {
        unsafe { ::telekio::IoResource::aio((self as *mut Self).cast(), configure::<T>) }
    }
}

unsafe extern "C" fn configure<T: AioSource>(
    data: *mut std::ffi::c_void,
    kqueue: i32,
    token: usize,
) -> ::telekio::CallResult {
    match catch_unwind(AssertUnwindSafe(|| {
        let source = unsafe { &mut *data.cast::<MioSource<T>>() };
        source.0.register_borrowed(
            unsafe { std::os::fd::BorrowedFd::borrow_raw(kqueue) },
            token,
        );
    })) {
        Ok(()) => ::telekio::CallResult::ok(),
        Err(payload) => ::telekio::CallResult::panicked(&*payload),
    }
}
