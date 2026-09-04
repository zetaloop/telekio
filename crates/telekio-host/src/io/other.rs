use super::*;

pub(super) struct Registration;

pub(super) fn register_inner(
    _: &HandleContext,
    _: IoResource,
    _: IoInterest,
) -> io::Result<Registration> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "this I/O resource is not supported on this platform",
    ))
}

pub(super) unsafe extern "C" fn poll_registration(
    _: *mut std::ffi::c_void,
    _: IoInterest,
    _: *const Waker,
) -> IoPoll {
    io_poll(Poll::Ready, call_ok(), IoReady::SHUTDOWN)
}

pub(super) unsafe extern "C" fn ready(
    _: *mut std::ffi::c_void,
    _: IoInterest,
) -> IoOperationResult {
    IoOperationResult {
        call: CallResult::ok(),
        operation: telekio::IoOperation::empty(),
    }
}

pub(super) unsafe extern "C" fn try_operate(_: *mut std::ffi::c_void, _: IoRequest) -> IoPoll {
    io_poll(Poll::Ready, call_ok(), IoReady::SHUTDOWN)
}

pub(super) unsafe extern "C" fn try_ready(_: *mut std::ffi::c_void, _: IoInterest) -> IoPoll {
    io_poll(Poll::Ready, call_ok(), IoReady::SHUTDOWN)
}

pub(super) unsafe extern "C" fn clear(_: *mut std::ffi::c_void, _: u8, _: IoReady) -> IoCallResult {
    IoCallResult {
        call: CallResult::ok(),
        error: IoError::none(),
    }
}
