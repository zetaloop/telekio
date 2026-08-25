use std::{
    io,
    panic::{AssertUnwindSafe, catch_unwind},
};

#[cfg(unix)]
use std::{future::Future, pin::Pin, sync::Arc};

use telekio::{
    CallResult, IoInterest, IoPoll, IoReady, IoRegistration, IoResource, IoResult, OwnedBytes,
    Poll, Status, Waker,
};

#[cfg(unix)]
use telekio::IoKind;

use super::HandleContext;

#[cfg(unix)]
type ReadyFuture = Pin<Box<dyn Future<Output = io::Result<tokio::io::Ready>> + Send>>;

#[cfg(unix)]
struct Registration {
    io: Arc<tokio::io::unix::AsyncFd<std::os::fd::BorrowedFd<'static>>>,
}

#[cfg(unix)]
struct Operation {
    future: ReadyFuture,
}

pub(super) unsafe extern "C" fn register(
    context: *const std::ffi::c_void,
    resource: IoResource,
    interest: IoInterest,
) -> IoResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    if !context.io_enabled {
        return IoResult {
            call: call_error(io::Error::other(telekio::IO_DRIVER_DISABLED_ERROR)),
            registration: IoRegistration::empty(),
        };
    }
    match catch_unwind(AssertUnwindSafe(|| {
        register_inner(context, resource, interest)
    })) {
        Ok(Ok(registration)) => IoResult {
            call: call_ok(),
            registration,
        },
        Ok(Err(error)) => IoResult {
            call: call_error(error),
            registration: IoRegistration::empty(),
        },
        Err(payload) => IoResult {
            call: super::host_panic(&*payload),
            registration: IoRegistration::empty(),
        },
    }
}

#[cfg(unix)]
fn register_inner(
    context: &HandleContext,
    resource: IoResource,
    interest: IoInterest,
) -> io::Result<IoRegistration> {
    use std::os::fd::{BorrowedFd, RawFd};

    if resource.kind != IoKind::Fd {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a Unix file descriptor",
        ));
    }
    let raw = resource.raw as u32 as RawFd;
    if raw < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid file descriptor",
        ));
    }
    let fd = unsafe { BorrowedFd::borrow_raw(raw) };
    let io = {
        let _guard = context.handle.enter();
        Arc::new(tokio::io::unix::AsyncFd::with_interest(
            fd,
            host_interest(interest)?,
        )?)
    };
    Ok(unsafe {
        IoRegistration::from_raw(
            Box::into_raw(Box::new(Registration { io })).cast(),
            poll_registration,
            ready,
            try_ready,
            clear,
            release_registration,
        )
    })
}

#[cfg(not(unix))]
fn register_inner(_: &HandleContext, _: IoResource, _: IoInterest) -> io::Result<IoRegistration> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "this I/O resource is not supported on this platform",
    ))
}

#[cfg(unix)]
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

#[cfg(unix)]
fn add_interest(target: &mut Option<tokio::io::Interest>, interest: tokio::io::Interest) {
    *target = Some(target.map_or(interest, |current| current | interest));
}

#[cfg(unix)]
unsafe extern "C" fn poll_registration(
    data: *mut std::ffi::c_void,
    interest: IoInterest,
    waker: *const Waker,
) -> IoPoll {
    let registration = unsafe { &*data.cast::<Registration>() };
    let waker = unsafe { (*waker).clone_rust_waker() };
    let mut context = std::task::Context::from_waker(&waker);
    let result = if interest.contains(IoInterest::WRITABLE) {
        registration.io.poll_write_ready(&mut context)
    } else {
        registration.io.poll_read_ready(&mut context)
    };
    match result {
        std::task::Poll::Pending => io_poll(Poll::Pending, call_ok(), IoReady::empty()),
        std::task::Poll::Ready(Ok(guard)) => {
            io_poll(Poll::Ready, call_ok(), map_ready(guard.ready()))
        }
        std::task::Poll::Ready(Err(error)) => {
            io_poll(Poll::Ready, call_error(error), IoReady::SHUTDOWN)
        }
    }
}

#[cfg(not(unix))]
#[expect(dead_code)]
unsafe extern "C" fn poll_registration(
    _: *mut std::ffi::c_void,
    _: IoInterest,
    _: *const Waker,
) -> IoPoll {
    io_poll(Poll::Ready, call_ok(), IoReady::SHUTDOWN)
}

#[cfg(unix)]
unsafe extern "C" fn ready(
    data: *mut std::ffi::c_void,
    interest: IoInterest,
) -> telekio::IoOperation {
    let registration = unsafe { &*data.cast::<Registration>() };
    let future = readiness(Arc::clone(&registration.io), interest);
    unsafe {
        telekio::IoOperation::from_raw(
            Box::into_raw(Box::new(Operation { future })).cast(),
            poll_operation,
            release_operation,
        )
    }
}

#[cfg(not(unix))]
#[expect(dead_code)]
unsafe extern "C" fn ready(_: *mut std::ffi::c_void, _: IoInterest) -> telekio::IoOperation {
    telekio::IoOperation::empty()
}

#[cfg(unix)]
fn readiness(
    io: Arc<tokio::io::unix::AsyncFd<std::os::fd::BorrowedFd<'static>>>,
    interest: IoInterest,
) -> ReadyFuture {
    match host_interest(interest) {
        Ok(interest) => {
            Box::pin(async move { io.ready(interest).await.map(|guard| guard.ready()) })
        }
        Err(error) => Box::pin(async move { Err(error) }),
    }
}

#[cfg(unix)]
unsafe extern "C" fn poll_operation(data: *mut std::ffi::c_void, waker: *const Waker) -> IoPoll {
    let operation = unsafe { &mut *data.cast::<Operation>() };
    poll_pinned(operation.future.as_mut(), waker)
}

#[cfg(unix)]
fn poll_pinned(
    future: Pin<&mut (dyn Future<Output = io::Result<tokio::io::Ready>> + Send)>,
    waker: *const Waker,
) -> IoPoll {
    let waker = unsafe { (*waker).clone_rust_waker() };
    let mut context = std::task::Context::from_waker(&waker);
    match future.poll(&mut context) {
        std::task::Poll::Pending => io_poll(Poll::Pending, call_ok(), IoReady::empty()),
        std::task::Poll::Ready(Ok(ready)) => io_poll(Poll::Ready, call_ok(), map_ready(ready)),
        std::task::Poll::Ready(Err(error)) => {
            io_poll(Poll::Ready, call_error(error), IoReady::SHUTDOWN)
        }
    }
}

#[cfg(unix)]
unsafe extern "C" fn try_ready(data: *mut std::ffi::c_void, interest: IoInterest) -> IoPoll {
    let registration = unsafe { &*data.cast::<Registration>() };
    match host_interest(interest) {
        Ok(interest) => match registration.io.try_io(interest, |_| Ok(())) {
            Ok(()) => io_poll(Poll::Ready, call_ok(), interest_ready(interest)),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                io_poll(Poll::Pending, call_ok(), IoReady::empty())
            }
            Err(error) => io_poll(Poll::Ready, call_error(error), IoReady::SHUTDOWN),
        },
        Err(error) => io_poll(Poll::Ready, call_error(error), IoReady::empty()),
    }
}

#[cfg(not(unix))]
#[expect(dead_code)]
unsafe extern "C" fn try_ready(_: *mut std::ffi::c_void, _: IoInterest) -> IoPoll {
    io_poll(Poll::Ready, call_ok(), IoReady::SHUTDOWN)
}

#[cfg(unix)]
fn interest_ready(interest: tokio::io::Interest) -> IoReady {
    let mut ready = IoReady::empty();
    if interest.is_readable() {
        ready |= IoReady::READABLE;
    }
    if interest.is_writable() {
        ready |= IoReady::WRITABLE;
    }
    if interest.is_error() {
        ready |= IoReady::ERROR;
    }
    #[cfg(any(target_os = "android", target_os = "linux"))]
    if interest.is_priority() {
        ready |= IoReady::PRIORITY;
    }
    ready
}

#[cfg(unix)]
unsafe extern "C" fn clear(data: *mut std::ffi::c_void, ready: IoReady) {
    let registration = unsafe { &*data.cast::<Registration>() };
    for interest in clear_interests(ready) {
        let _ = registration
            .io
            .try_io(interest, |_| Err::<(), _>(io::ErrorKind::WouldBlock.into()));
    }
}

#[cfg(not(unix))]
#[expect(dead_code)]
unsafe extern "C" fn clear(_: *mut std::ffi::c_void, _: IoReady) {}

#[cfg(unix)]
unsafe extern "C" fn release_registration(data: *mut std::ffi::c_void) {
    drop(unsafe { Box::from_raw(data.cast::<Registration>()) });
}

#[cfg(not(unix))]
#[expect(dead_code)]
unsafe extern "C" fn release_registration(_: *mut std::ffi::c_void) {}

#[cfg(unix)]
unsafe extern "C" fn release_operation(data: *mut std::ffi::c_void) {
    drop(unsafe { Box::from_raw(data.cast::<Operation>()) });
}

#[cfg(unix)]
fn clear_interests(ready: IoReady) -> Vec<tokio::io::Interest> {
    let mut interests = Vec::new();
    if ready.contains(IoReady::READABLE) || ready.contains(IoReady::READ_CLOSED) {
        interests.push(tokio::io::Interest::READABLE);
    }
    if ready.contains(IoReady::WRITABLE) || ready.contains(IoReady::WRITE_CLOSED) {
        interests.push(tokio::io::Interest::WRITABLE);
    }
    if ready.contains(IoReady::ERROR) {
        interests.push(tokio::io::Interest::ERROR);
    }
    #[cfg(any(target_os = "android", target_os = "linux"))]
    if ready.contains(IoReady::PRIORITY) {
        interests.push(tokio::io::Interest::PRIORITY);
    }
    interests
}

#[cfg(unix)]
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

fn io_poll(state: Poll, call: CallResult, ready: IoReady) -> IoPoll {
    IoPoll { state, call, ready }
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
