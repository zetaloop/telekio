use std::{
    io,
    panic::{AssertUnwindSafe, catch_unwind},
};

use std::sync::Arc;
#[cfg(any(unix, windows))]
use std::{future::Future, pin::Pin};

use telekio::{
    CallResult, Callback, IoCallResult, IoDriverRegistration, IoDriverResult, IoError, IoInterest,
    IoOperationResult, IoPoll, IoReady, IoRegistration, IoRequest, IoResource, IoResult,
    OwnedBytes, Poll, Status, Waker,
};

#[cfg(any(unix, windows))]
use telekio::IoKind;

#[cfg(target_os = "linux")]
use super::{CallbackOwner, host_callback};
use super::{HandleContext, HostResource};

#[cfg(unix)]
#[path = "io/unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "io/windows.rs"]
mod imp;
#[cfg(not(any(unix, windows)))]
#[path = "io/other.rs"]
mod imp;

use imp::{Registration, register_inner};
#[cfg(not(any(unix, windows)))]
use imp::{clear, poll_registration, ready, try_operate, try_ready};

#[cfg(any(unix, windows))]
type OperationFuture = Pin<Box<dyn Future<Output = io::Result<HostReady>> + Send>>;

#[cfg(any(unix, windows))]
struct HostReady {
    tick: u8,
    ready: IoReady,
}

#[cfg(any(unix, windows))]
struct Operation {
    future: OperationFuture,
}

#[cfg(target_os = "linux")]
struct DriverResource {
    _registration: tokio::runtime::telekio::TelekioIo,
    _callback: Arc<CallbackOwner>,
}

pub(super) unsafe extern "C" fn register(
    context: *const std::ffi::c_void,
    resource: IoResource,
    interest: IoInterest,
) -> IoResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    if !context.io_enabled {
        let error = io::Error::other(telekio::IO_DRIVER_DISABLED_ERROR);
        return IoResult {
            error: IoError::from_error(&error),
            call: call_error(error),
            registration: IoRegistration::empty(),
        };
    }
    match catch_unwind(AssertUnwindSafe(|| {
        let registration = register_inner(context, resource, interest)?;
        HostResource::new(&context.owner, registration).map_err(io::Error::other)
    })) {
        Ok(Ok(registration)) => IoResult {
            call: call_ok(),
            error: IoError::none(),
            registration: unsafe {
                IoRegistration::from_raw(
                    Arc::as_ptr(&registration).cast_mut().cast(),
                    poll_registration,
                    ready,
                    try_operate,
                    try_ready,
                    clear,
                    release_registration,
                )
            },
        },
        Ok(Err(error)) => IoResult {
            error: IoError::from_error(&error),
            call: call_error(error),
            registration: IoRegistration::empty(),
        },
        Err(payload) => IoResult {
            call: super::host_panic(&*payload),
            error: IoError::none(),
            registration: IoRegistration::empty(),
        },
    }
}

#[cfg(target_os = "linux")]
pub(super) unsafe extern "C" fn register_driver(
    context: *const std::ffi::c_void,
    resource: IoResource,
    callback: Callback,
) -> IoDriverResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| -> io::Result<_> {
        if !context.io_enabled {
            return Err(io::Error::other(telekio::IO_DRIVER_DISABLED_ERROR));
        }
        if resource.kind() != IoKind::Fd {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a Linux file descriptor",
            ));
        }
        let callback = Arc::new(CallbackOwner(callback));
        let weak = Arc::downgrade(&callback);
        let owner = Arc::downgrade(&context.owner);
        let registration = context.handle.telekio_register_io(
            resource.raw() as u32 as i32,
            Arc::new(move || {
                let Some(owner) = owner.upgrade() else {
                    return;
                };
                let Some(_activity) = owner.callback(std::task::Waker::noop().clone()) else {
                    return;
                };
                if let Some(callback) = weak.upgrade() {
                    callback.call();
                }
            }),
        )?;
        let resource = HostResource::new(
            &context.owner,
            DriverResource {
                _registration: registration,
                _callback: callback,
            },
        )
        .map_err(io::Error::other)?;
        Ok(unsafe {
            IoDriverRegistration::from_raw(Arc::as_ptr(&resource).cast_mut().cast(), release_driver)
        })
    })) {
        Ok(Ok(registration)) => IoDriverResult {
            call: call_ok(),
            error: IoError::none(),
            registration,
        },
        Ok(Err(error)) => IoDriverResult {
            error: IoError::from_error(&error),
            call: call_error(error),
            registration: IoDriverRegistration::empty(),
        },
        Err(payload) => IoDriverResult {
            call: super::host_panic(&*payload),
            error: IoError::none(),
            registration: IoDriverRegistration::empty(),
        },
    }
}

#[cfg(not(target_os = "linux"))]
pub(super) unsafe extern "C" fn register_driver(
    _: *const std::ffi::c_void,
    _: IoResource,
    callback: Callback,
) -> IoDriverResult {
    match catch_unwind(AssertUnwindSafe(|| drop(callback))) {
        Ok(()) => {
            let error = io::Error::new(
                io::ErrorKind::Unsupported,
                "unsupported I/O driver registration",
            );
            IoDriverResult {
                error: IoError::from_error(&error),
                call: call_error(error),
                registration: IoDriverRegistration::empty(),
            }
        }
        Err(payload) => IoDriverResult {
            call: super::host_panic(&*payload),
            error: IoError::none(),
            registration: IoDriverRegistration::empty(),
        },
    }
}

#[cfg(target_os = "linux")]
unsafe extern "C" fn release_driver(data: *mut std::ffi::c_void) -> CallResult {
    host_callback(|| {
        let registration = unsafe { &*data.cast::<HostResource<DriverResource>>() };
        registration.release();
    })
}

#[cfg(any(unix, windows))]
fn host_interest(interest: IoInterest) -> io::Result<tokio::io::Interest> {
    let mut result = None;
    if interest.contains(IoInterest::READABLE) {
        add_interest(&mut result, tokio::io::Interest::READABLE);
    }
    if interest.contains(IoInterest::WRITABLE) {
        add_interest(&mut result, tokio::io::Interest::WRITABLE);
    }
    if interest.contains(IoInterest::ERROR) {
        add_interest(&mut result, tokio::io::Interest::ERROR);
    }
    #[cfg(any(target_os = "android", target_os = "linux"))]
    if interest.contains(IoInterest::PRIORITY) {
        add_interest(&mut result, tokio::io::Interest::PRIORITY);
    }
    result.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "I/O interest is empty"))
}

#[cfg(any(unix, windows))]
fn add_interest(target: &mut Option<tokio::io::Interest>, interest: tokio::io::Interest) {
    *target = Some(target.map_or(interest, |current| current | interest));
}

#[cfg(any(unix, windows))]
unsafe extern "C" fn poll_registration(
    data: *mut std::ffi::c_void,
    interest: IoInterest,
    waker: *const Waker,
) -> IoPoll {
    io_callback(|| {
        let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
        registration.update_waker(unsafe { &*waker });
        let waker = unsafe { (*waker).clone_rust_waker() };
        let mut context = std::task::Context::from_waker(&waker);
        registration
            .with(|registration| registration.poll_ready(&mut context, interest))
            .map(ready_poll)
            .unwrap_or_else(resource_error)
    })
}

#[cfg(any(unix, windows))]
unsafe extern "C" fn try_operate(data: *mut std::ffi::c_void, request: IoRequest) -> IoPoll {
    registration_call(data, |registration| registration.try_operate(request))
}

#[cfg(any(unix, windows))]
unsafe extern "C" fn try_ready(data: *mut std::ffi::c_void, interest: IoInterest) -> IoPoll {
    registration_call(data, |registration| registration.try_ready(interest))
}

#[cfg(any(unix, windows))]
unsafe extern "C" fn clear(data: *mut std::ffi::c_void, tick: u8, ready: IoReady) -> IoCallResult {
    io_call(|| {
        let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
        if let Ok(result) = registration.with(|registration| registration.clear(tick, ready)) {
            result?;
        }
        Ok(())
    })
}

#[cfg(any(unix, windows))]
unsafe extern "C" fn ready(data: *mut std::ffi::c_void, interest: IoInterest) -> IoOperationResult {
    match catch_unwind(AssertUnwindSafe(|| {
        let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
        let owner = Arc::clone(&registration.owner);
        let future = registration.with(|registration| registration.ready(interest))?;
        let operation = HostResource::new(&owner, Operation { future })?;
        Ok::<_, String>(unsafe {
            telekio::IoOperation::from_raw(
                Arc::as_ptr(&operation).cast_mut().cast(),
                poll_operation,
                release_operation,
            )
        })
    })) {
        Ok(Ok(operation)) => IoOperationResult {
            call: CallResult::ok(),
            operation,
        },
        Ok(Err(error)) => IoOperationResult {
            call: CallResult {
                status: Status::Error,
                payload: OwnedBytes::from_string(error),
            },
            operation: telekio::IoOperation::empty(),
        },
        Err(payload) => IoOperationResult {
            call: super::host_panic(&*payload),
            operation: telekio::IoOperation::empty(),
        },
    }
}

#[cfg(any(unix, windows))]
unsafe extern "C" fn poll_operation(data: *mut std::ffi::c_void, waker: *const Waker) -> IoPoll {
    io_callback(|| {
        let operation = unsafe { &*data.cast::<HostResource<Operation>>() };
        operation.update_waker(unsafe { &*waker });
        operation
            .with_mut(|operation| poll_completion(operation.future.as_mut(), waker))
            .unwrap_or_else(resource_error)
    })
}

#[cfg(any(unix, windows))]
fn poll_completion(
    future: Pin<&mut (dyn Future<Output = io::Result<HostReady>> + Send)>,
    waker: *const Waker,
) -> IoPoll {
    let waker = unsafe { (*waker).clone_rust_waker() };
    let mut context = std::task::Context::from_waker(&waker);
    completion_poll(future.poll(&mut context))
}

#[cfg(any(unix, windows))]
fn completion_poll(result: std::task::Poll<io::Result<HostReady>>) -> IoPoll {
    match result {
        std::task::Poll::Pending => io_poll(Poll::Pending, call_ok(), IoReady::empty()),
        std::task::Poll::Ready(Ok(ready)) => {
            io_poll_event(Poll::Ready, call_ok(), ready.tick, ready.ready)
        }
        std::task::Poll::Ready(Err(error)) => io_poll_error(Poll::Ready, error, IoReady::SHUTDOWN),
    }
}

#[cfg(any(unix, windows))]
fn ready_now(ready: HostReady) -> IoPoll {
    if ready.ready.bits() == 0 {
        io_poll(Poll::Pending, call_ok(), IoReady::empty())
    } else {
        io_poll_event(Poll::Ready, call_ok(), ready.tick, ready.ready)
    }
}

unsafe extern "C" fn release_registration(data: *mut std::ffi::c_void) -> CallResult {
    super::host_callback(|| {
        unsafe { &*data.cast::<HostResource<Registration>>() }.release();
    })
}

#[cfg(any(unix, windows))]
unsafe extern "C" fn release_operation(data: *mut std::ffi::c_void) -> CallResult {
    super::host_callback(|| {
        unsafe { &*data.cast::<HostResource<Operation>>() }.release();
    })
}

#[cfg(any(unix, windows))]
fn host_ready((tick, ready, shutdown): (u8, tokio::io::Ready, bool)) -> HostReady {
    let mut ready = map_ready(ready);
    if shutdown {
        ready |= IoReady::SHUTDOWN;
    }
    HostReady { tick, ready }
}

#[cfg(any(unix, windows))]
fn ready_poll(result: std::task::Poll<io::Result<(u8, tokio::io::Ready, bool)>>) -> IoPoll {
    match result {
        std::task::Poll::Pending => io_poll(Poll::Pending, call_ok(), IoReady::empty()),
        std::task::Poll::Ready(Ok(ready)) => ready_now(host_ready(ready)),
        std::task::Poll::Ready(Err(error)) => io_poll_error(Poll::Ready, error, IoReady::SHUTDOWN),
    }
}

#[cfg(any(unix, windows))]
fn map_ready(ready: tokio::io::Ready) -> IoReady {
    let mut result = IoReady::empty();
    if ready.is_readable() {
        result |= IoReady::READABLE;
    }
    if ready.is_writable() {
        result |= IoReady::WRITABLE;
    }
    if ready.is_read_closed() {
        result |= IoReady::READ_CLOSED;
    }
    if ready.is_write_closed() {
        result |= IoReady::WRITE_CLOSED;
    }
    if ready.is_error() {
        result |= IoReady::ERROR;
    }
    #[cfg(any(target_os = "android", target_os = "linux"))]
    if ready.is_priority() {
        result |= IoReady::PRIORITY;
    }
    result
}

#[cfg(any(unix, windows))]
fn tokio_ready(ready: IoReady) -> tokio::io::Ready {
    let mut result = tokio::io::Ready::EMPTY;
    if ready.contains(IoReady::READABLE) {
        result |= tokio::io::Ready::READABLE;
    }
    if ready.contains(IoReady::WRITABLE) {
        result |= tokio::io::Ready::WRITABLE;
    }
    if ready.contains(IoReady::READ_CLOSED) {
        result |= tokio::io::Ready::READ_CLOSED;
    }
    if ready.contains(IoReady::WRITE_CLOSED) {
        result |= tokio::io::Ready::WRITE_CLOSED;
    }
    if ready.contains(IoReady::ERROR) {
        result |= tokio::io::Ready::ERROR;
    }
    #[cfg(any(target_os = "android", target_os = "linux"))]
    if ready.contains(IoReady::PRIORITY) {
        result |= tokio::io::Ready::PRIORITY;
    }
    result
}

fn io_poll(state: Poll, call: CallResult, ready: IoReady) -> IoPoll {
    io_poll_value(state, call, ready, 0)
}

fn io_poll_event(state: Poll, call: CallResult, tick: u8, ready: IoReady) -> IoPoll {
    IoPoll {
        state,
        call,
        error: IoError::none(),
        ready,
        tick,
        value: 0,
    }
}

fn io_poll_value(state: Poll, call: CallResult, ready: IoReady, value: usize) -> IoPoll {
    IoPoll {
        state,
        call,
        error: IoError::none(),
        ready,
        tick: 0,
        value,
    }
}

fn io_poll_error(state: Poll, error: io::Error, ready: IoReady) -> IoPoll {
    IoPoll {
        state,
        error: IoError::from_error(&error),
        call: call_error(error),
        ready,
        tick: 0,
        value: 0,
    }
}

#[cfg(any(unix, windows))]
fn registration_call(
    data: *mut std::ffi::c_void,
    call: impl FnOnce(&Registration) -> IoPoll,
) -> IoPoll {
    io_callback(|| {
        let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
        registration.with(call).unwrap_or_else(resource_error)
    })
}

fn resource_error(error: String) -> IoPoll {
    io_poll_error(Poll::Ready, io::Error::other(error), IoReady::SHUTDOWN)
}

fn io_callback(call: impl FnOnce() -> IoPoll) -> IoPoll {
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(result) => result,
        Err(payload) => io_poll(
            Poll::Panicked,
            super::host_panic(&*payload),
            IoReady::empty(),
        ),
    }
}

fn io_call(call: impl FnOnce() -> io::Result<()>) -> IoCallResult {
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(Ok(())) => IoCallResult {
            call: call_ok(),
            error: IoError::none(),
        },
        Ok(Err(error)) => IoCallResult {
            error: IoError::from_error(&error),
            call: call_error(error),
        },
        Err(payload) => IoCallResult {
            call: super::host_panic(&*payload),
            error: IoError::none(),
        },
    }
}

fn call_ok() -> CallResult {
    CallResult {
        status: Status::Ok,
        payload: OwnedBytes::empty(),
    }
}

fn call_error(error: io::Error) -> CallResult {
    CallResult {
        status: Status::Error,
        payload: OwnedBytes::from_string(error.to_string()),
    }
}
