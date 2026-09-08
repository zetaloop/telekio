#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn telekio_guest_context(
    raw: ::telekio_abi::RawHandle,
) -> ::telekio_abi::AttachResult {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let handle = unsafe { ::telekio_abi::Handle::from_abi(raw) };
        if let Err(handle) = ::telekio_abi::install_handle(handle) {
            drop(handle);
            return Err("Telekio runtime is already attached");
        }
        Ok(unsafe {
            ::telekio_abi::RawAttachment::from_raw(
                std::ptr::null_mut(),
                enter_guest_context,
                detach_guest_context,
            )
        })
    })) {
        Ok(Ok(attachment)) => ::telekio_abi::AttachResult {
            call: ::telekio_abi::CallResult::ok(),
            attachment,
        },
        Ok(Err(error)) => ::telekio_abi::AttachResult {
            call: ::telekio_abi::CallResult::error(error),
            attachment: ::telekio_abi::RawAttachment::empty(),
        },
        Err(payload) => ::telekio_abi::AttachResult {
            call: ::telekio_abi::CallResult::panicked(&*payload),
            attachment: ::telekio_abi::RawAttachment::empty(),
        },
    }
}

unsafe extern "C" fn enter_guest_context(
    _: *mut std::ffi::c_void,
    execution: *mut ::telekio_abi::ExecutionState,
    call: ::telekio_abi::GuestCall,
) -> ::telekio_abi::CallResult {
    unsafe { ::telekio_abi::with_execution_state(execution, || call.invoke()) }
}

unsafe extern "C" fn detach_guest_context(_: *mut std::ffi::c_void) -> ::telekio_abi::CallResult {
    unsafe { ::telekio_abi::detach_attached() }
}
