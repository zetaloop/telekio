use super::*;
use crate::runtime::io::telekio::Source;
use std::panic::{catch_unwind, AssertUnwindSafe};

impl<T: AioSource> Source for MioSource<T> {
    fn telekio_resource(&mut self) -> ::telekio_abi::IoResource {
        unsafe { ::telekio_abi::IoResource::aio((self as *mut Self).cast(), configure::<T>) }
    }
}

unsafe extern "C" fn configure<T: AioSource>(
    data: *mut std::ffi::c_void,
    kqueue: i32,
    token: usize,
) -> ::telekio_abi::CallResult {
    match catch_unwind(AssertUnwindSafe(|| {
        let source = unsafe { &mut *data.cast::<MioSource<T>>() };
        source.0.register_borrowed(
            unsafe { std::os::fd::BorrowedFd::borrow_raw(kqueue) },
            token,
        );
    })) {
        Ok(()) => ::telekio_abi::CallResult::ok(),
        Err(payload) => ::telekio_abi::CallResult::panicked(&*payload),
    }
}
