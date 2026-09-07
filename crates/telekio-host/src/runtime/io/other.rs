use std::{ffi::c_void, io};

use telekio::{
    CallResult, Callback, IoDriverRegistration, IoDriverResult, IoError, IoInterest,
    IoRegistration, IoResource, IoResult, Status,
};

pub(super) unsafe extern "C" fn register(
    _: *const c_void,
    _: IoResource,
    _: IoInterest,
) -> IoResult {
    let error = io::Error::new(
        io::ErrorKind::Unsupported,
        "Tokio host I/O is unavailable in this configuration",
    );
    IoResult {
        call: CallResult::error(&error.to_string()),
        error: IoError::from_error(&error),
        registration: IoRegistration::empty(),
    }
}

pub(super) unsafe extern "C" fn register_driver(
    _: *const c_void,
    _: IoResource,
    callback: Callback,
) -> IoDriverResult {
    let call = crate::host_callback(|| drop(callback));
    let error = io::Error::new(
        io::ErrorKind::Unsupported,
        "Tokio host I/O driver registration is unavailable in this configuration",
    );
    IoDriverResult {
        call: if call.status == Status::Ok {
            CallResult::error(&error.to_string())
        } else {
            call
        },
        error: IoError::from_error(&error),
        registration: IoDriverRegistration::empty(),
    }
}
