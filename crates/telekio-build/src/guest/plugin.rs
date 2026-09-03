#[unsafe(no_mangle)]
pub(super) extern "C-unwind" fn telekio_default_handle() -> ::telekio::RawHandle {
    ::telekio::RawHandle::empty()
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn telekio_guest_context(
    raw: ::telekio::RawHandle,
) -> ::telekio::AttachResult {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let handle = unsafe { ::telekio::Handle::from_abi(raw) };
        if let Err(handle) = ::telekio::install_handle(handle) {
            drop(handle);
            return Err("Telekio runtime is already attached");
        }
        Ok(unsafe {
            ::telekio::RawAttachment::from_raw(
                std::ptr::null_mut(),
                enter_guest_context,
                detach_guest_context,
            )
        })
    })) {
        Ok(Ok(attachment)) => ::telekio::AttachResult {
            call: ::telekio::CallResult::ok(),
            attachment,
        },
        Ok(Err(error)) => ::telekio::AttachResult {
            call: ::telekio::CallResult::error(error),
            attachment: ::telekio::RawAttachment::empty(),
        },
        Err(payload) => ::telekio::AttachResult {
            call: ::telekio::CallResult::panicked(&*payload),
            attachment: ::telekio::RawAttachment::empty(),
        },
    }
}

unsafe extern "C" fn enter_guest_context(
    _: *mut std::ffi::c_void,
    execution: *mut ::telekio::ExecutionState,
    call: ::telekio::GuestCall,
) -> ::telekio::CallResult {
    unsafe { ::telekio::with_execution_state(execution, || call.invoke()) }
}

unsafe extern "C" fn detach_guest_context(_: *mut std::ffi::c_void) -> ::telekio::CallResult {
    unsafe { ::telekio::detach_attached() }
}
