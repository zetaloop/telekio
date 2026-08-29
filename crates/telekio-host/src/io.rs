use std::{
    io,
    panic::{AssertUnwindSafe, catch_unwind},
};

#[cfg(windows)]
use std::sync::Mutex;
#[cfg(any(unix, windows))]
use std::{future::Future, pin::Pin, sync::Arc};

use telekio::{
    CallResult, IoError, IoInterest, IoPoll, IoReady, IoRegistration, IoRequest, IoResource,
    IoResult, OwnedBytes, Poll, Status, Waker,
};

#[cfg(any(unix, windows))]
use telekio::IoKind;
#[cfg(windows)]
use telekio::IoOperationKind;

use super::{HandleContext, HostResource};

#[cfg(windows)]
type ReadyFuture = Pin<Box<dyn Future<Output = io::Result<tokio::io::Ready>> + Send>>;

#[cfg(any(unix, windows))]
type OperationFuture = Pin<Box<dyn Future<Output = io::Result<IoReady>> + Send>>;

#[cfg(not(any(unix, windows)))]
struct Registration;

#[cfg(unix)]
struct Registration {
    io: Arc<tokio::io::unix::AsyncFd<std::os::fd::BorrowedFd<'static>>>,
}

#[cfg(windows)]
#[derive(Clone)]
enum WindowsIo {
    Socket(Arc<tokio::net::UdpSocket>),
    Pipe(Arc<tokio::net::windows::named_pipe::NamedPipeServer>),
}

#[cfg(windows)]
impl WindowsIo {
    async fn ready(&self, interest: tokio::io::Interest) -> io::Result<tokio::io::Ready> {
        match self {
            Self::Socket(io) => io.ready(interest).await,
            Self::Pipe(io) => io.ready(interest).await,
        }
    }

    fn try_io<R>(
        &self,
        interest: tokio::io::Interest,
        call: impl FnOnce() -> io::Result<R>,
    ) -> io::Result<R> {
        match self {
            Self::Socket(io) => io.try_io(interest, call),
            Self::Pipe(io) => io.try_io(interest, call),
        }
    }
}

#[cfg(windows)]
struct Registration {
    io: WindowsIo,
    read: Mutex<Option<ReadyFuture>>,
    write: Mutex<Option<ReadyFuture>>,
    connect: Mutex<Option<OperationFuture>>,
}

#[cfg(any(unix, windows))]
struct Operation {
    future: OperationFuture,
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
                    Arc::into_raw(registration).cast_mut().cast(),
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

#[cfg(unix)]
fn register_inner(
    context: &HandleContext,
    resource: IoResource,
    interest: IoInterest,
) -> io::Result<Registration> {
    use std::os::fd::{BorrowedFd, RawFd};

    if resource.kind() != IoKind::Fd {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a Unix file descriptor",
        ));
    }
    let raw = resource.raw() as u32 as RawFd;
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
    Ok(Registration { io })
}

#[cfg(windows)]
fn register_inner(
    context: &HandleContext,
    resource: IoResource,
    _: IoInterest,
) -> io::Result<Registration> {
    use std::os::windows::io::{
        BorrowedHandle, BorrowedSocket, IntoRawHandle, RawHandle, RawSocket,
    };

    let io = match resource.kind() {
        IoKind::Socket => {
            let socket = unsafe { BorrowedSocket::borrow_raw(resource.raw() as RawSocket) }
                .try_clone_to_owned()?;
            let socket = std::net::UdpSocket::from(socket);
            let _guard = context.handle.enter();
            WindowsIo::Socket(Arc::new(tokio::net::UdpSocket::from_std(socket)?))
        }
        IoKind::Handle => {
            let raw = resource.raw() as usize as RawHandle;
            let handle = unsafe { BorrowedHandle::borrow_raw(raw) }.try_clone_to_owned()?;
            let _guard = context.handle.enter();
            WindowsIo::Pipe(Arc::new(unsafe {
                tokio::net::windows::named_pipe::NamedPipeServer::from_raw_handle(
                    handle.into_raw_handle(),
                )?
            }))
        }
        IoKind::Fd => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a Windows socket or handle",
            ));
        }
    };
    Ok(Registration {
        io,
        read: Mutex::new(None),
        write: Mutex::new(None),
        connect: Mutex::new(None),
    })
}

#[cfg(not(any(unix, windows)))]
fn register_inner(_: &HandleContext, _: IoResource, _: IoInterest) -> io::Result<Registration> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "this I/O resource is not supported on this platform",
    ))
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

#[cfg(unix)]
unsafe extern "C" fn poll_registration(
    data: *mut std::ffi::c_void,
    interest: IoInterest,
    waker: *const Waker,
) -> IoPoll {
    let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
    registration.update_waker(unsafe { &*waker });
    let waker = unsafe { (*waker).clone_rust_waker() };
    let mut context = std::task::Context::from_waker(&waker);
    let result = registration.with(|registration| {
        let ready = if interest.contains(IoInterest::WRITABLE) {
            registration.io.poll_write_ready(&mut context)
        } else {
            registration.io.poll_read_ready(&mut context)
        };
        ready.map(|ready| ready.map(|guard| map_ready(guard.ready())))
    });
    match result {
        Err(error) => io_poll_error(Poll::Ready, io::Error::other(error), IoReady::SHUTDOWN),
        Ok(result) => match result {
            std::task::Poll::Pending => io_poll(Poll::Pending, call_ok(), IoReady::empty()),
            std::task::Poll::Ready(Ok(ready)) => io_poll(Poll::Ready, call_ok(), ready),
            std::task::Poll::Ready(Err(error)) => {
                io_poll_error(Poll::Ready, error, IoReady::SHUTDOWN)
            }
        },
    }
}

#[cfg(windows)]
unsafe extern "C" fn poll_registration(
    data: *mut std::ffi::c_void,
    interest: IoInterest,
    waker: *const Waker,
) -> IoPoll {
    let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
    registration.update_waker(unsafe { &*waker });
    registration
        .with(|registration| poll_windows_registration(registration, interest, waker))
        .unwrap_or_else(|error| {
            io_poll_error(Poll::Ready, io::Error::other(error), IoReady::SHUTDOWN)
        })
}

#[cfg(windows)]
fn poll_windows_registration(
    registration: &Registration,
    interest: IoInterest,
    waker: *const Waker,
) -> IoPoll {
    if let WindowsIo::Socket(io) = &registration.io {
        let slot = if interest.contains(IoInterest::WRITABLE) {
            &registration.write
        } else {
            &registration.read
        };
        let mut future = slot.lock().unwrap();
        if future.is_none() {
            *future = Some(socket_readiness(Arc::clone(io), interest));
        }
        let result = poll_ready(future.as_mut().unwrap().as_mut(), waker);
        if result.state == Poll::Ready {
            *future = None;
        }
        return result;
    }

    let WindowsIo::Pipe(pipe) = &registration.io else {
        unreachable!()
    };
    let waker = unsafe { (*waker).clone_rust_waker() };
    let mut context = std::task::Context::from_waker(&waker);
    let result = if interest.contains(IoInterest::WRITABLE) {
        pipe.poll_write_ready(&mut context)
    } else {
        pipe.poll_read_ready(&mut context)
    };
    match result {
        std::task::Poll::Pending => io_poll(Poll::Pending, call_ok(), IoReady::empty()),
        std::task::Poll::Ready(Ok(())) => {
            let ready = if interest.contains(IoInterest::WRITABLE) {
                IoReady::WRITABLE
            } else {
                IoReady::READABLE
            };
            io_poll(Poll::Ready, call_ok(), ready)
        }
        std::task::Poll::Ready(Err(error)) => io_poll_error(Poll::Ready, error, IoReady::SHUTDOWN),
    }
}

#[cfg(not(any(unix, windows)))]
#[expect(dead_code)]
unsafe extern "C" fn poll_registration(
    _: *mut std::ffi::c_void,
    _: IoInterest,
    _: *const Waker,
) -> IoPoll {
    io_poll(Poll::Ready, call_ok(), IoReady::SHUTDOWN)
}

#[cfg(any(unix, windows))]
unsafe extern "C" fn ready(
    data: *mut std::ffi::c_void,
    interest: IoInterest,
) -> telekio::IoOperation {
    let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
    let Some(owner) = registration.owner.upgrade() else {
        return telekio::IoOperation::empty();
    };
    let Ok(future) =
        registration.with(|registration| operation_ready(registration.io.clone(), interest))
    else {
        return telekio::IoOperation::empty();
    };
    let Ok(operation) = HostResource::new(&owner, Operation { future }) else {
        return telekio::IoOperation::empty();
    };
    unsafe {
        telekio::IoOperation::from_raw(
            Arc::into_raw(operation).cast_mut().cast(),
            poll_operation,
            release_operation,
        )
    }
}

#[cfg(not(any(unix, windows)))]
#[expect(dead_code)]
unsafe extern "C" fn ready(_: *mut std::ffi::c_void, _: IoInterest) -> telekio::IoOperation {
    telekio::IoOperation::empty()
}

#[cfg(windows)]
fn socket_readiness(io: Arc<tokio::net::UdpSocket>, interest: IoInterest) -> ReadyFuture {
    match host_interest(interest) {
        Ok(interest) => Box::pin(async move { io.ready(interest).await }),
        Err(error) => Box::pin(async move { Err(error) }),
    }
}

#[cfg(windows)]
fn poll_ready(
    future: Pin<&mut (dyn Future<Output = io::Result<tokio::io::Ready>> + Send)>,
    waker: *const Waker,
) -> IoPoll {
    let waker = unsafe { (*waker).clone_rust_waker() };
    let mut context = std::task::Context::from_waker(&waker);
    match future.poll(&mut context) {
        std::task::Poll::Pending => io_poll(Poll::Pending, call_ok(), IoReady::empty()),
        std::task::Poll::Ready(Ok(ready)) => io_poll(Poll::Ready, call_ok(), map_ready(ready)),
        std::task::Poll::Ready(Err(error)) => io_poll_error(Poll::Ready, error, IoReady::SHUTDOWN),
    }
}

#[cfg(any(unix, windows))]
unsafe extern "C" fn poll_operation(data: *mut std::ffi::c_void, waker: *const Waker) -> IoPoll {
    let operation = unsafe { &*data.cast::<HostResource<Operation>>() };
    operation.update_waker(unsafe { &*waker });
    operation
        .with_mut(|operation| poll_completion(operation.future.as_mut(), waker))
        .unwrap_or_else(|error| {
            io_poll_error(Poll::Ready, io::Error::other(error), IoReady::SHUTDOWN)
        })
}

#[cfg(any(unix, windows))]
fn poll_completion(
    future: Pin<&mut (dyn Future<Output = io::Result<IoReady>> + Send)>,
    waker: *const Waker,
) -> IoPoll {
    let waker = unsafe { (*waker).clone_rust_waker() };
    let mut context = std::task::Context::from_waker(&waker);
    completion_poll(future.poll(&mut context))
}

#[cfg(windows)]
fn poll_completion_now(
    future: Pin<&mut (dyn Future<Output = io::Result<IoReady>> + Send)>,
) -> IoPoll {
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    completion_poll(future.poll(&mut context))
}

#[cfg(any(unix, windows))]
fn completion_poll(result: std::task::Poll<io::Result<IoReady>>) -> IoPoll {
    match result {
        std::task::Poll::Pending => io_poll(Poll::Pending, call_ok(), IoReady::empty()),
        std::task::Poll::Ready(Ok(ready)) => io_poll(Poll::Ready, call_ok(), ready),
        std::task::Poll::Ready(Err(error)) => io_poll_error(Poll::Ready, error, IoReady::SHUTDOWN),
    }
}

#[cfg(any(unix, windows))]
fn operation_error(error: io::Error) -> OperationFuture {
    Box::pin(async move { Err(error) })
}

#[cfg(unix)]
fn operation_ready(
    io: Arc<tokio::io::unix::AsyncFd<std::os::fd::BorrowedFd<'static>>>,
    interest: IoInterest,
) -> OperationFuture {
    match host_interest(interest) {
        Ok(interest) => Box::pin(async move {
            io.ready(interest)
                .await
                .map(|guard| map_ready(guard.ready()))
        }),
        Err(error) => operation_error(error),
    }
}

#[cfg(windows)]
fn operation_ready(io: WindowsIo, interest: IoInterest) -> OperationFuture {
    match host_interest(interest) {
        Ok(interest) => Box::pin(async move { io.ready(interest).await.map(map_ready) }),
        Err(error) => operation_error(error),
    }
}

#[cfg(windows)]
fn connect_operation(io: WindowsIo) -> OperationFuture {
    match io {
        WindowsIo::Pipe(pipe) => {
            Box::pin(async move { pipe.connect().await.map(|_| IoReady::empty()) })
        }
        WindowsIo::Socket(_) => operation_error(io::Error::new(
            io::ErrorKind::Unsupported,
            "only a named-pipe server can connect",
        )),
    }
}

#[cfg(windows)]
fn pipe_error(error: io::Error) -> IoPoll {
    io_poll_error(Poll::Ready, error, IoReady::SHUTDOWN)
}

#[cfg(windows)]
unsafe extern "C" fn try_operate(data: *mut std::ffi::c_void, request: IoRequest) -> IoPoll {
    let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
    registration
        .with(|registration| try_operate_inner(registration, request))
        .unwrap_or_else(|error| {
            io_poll_error(Poll::Ready, io::Error::other(error), IoReady::SHUTDOWN)
        })
}

#[cfg(windows)]
fn try_operate_inner(registration: &Registration, request: IoRequest) -> IoPoll {
    let WindowsIo::Pipe(pipe) = &registration.io else {
        return pipe_error(io::Error::new(
            io::ErrorKind::Unsupported,
            "socket operation is guest-owned",
        ));
    };
    if request.buffer().is_null() && request.buffer_len() != 0 {
        return pipe_error(io::Error::new(
            io::ErrorKind::InvalidInput,
            "I/O buffer is null",
        ));
    }
    match request.kind() {
        IoOperationKind::Read => {
            let buffer =
                unsafe { std::slice::from_raw_parts_mut(request.buffer(), request.buffer_len()) };
            match pipe.try_read(buffer) {
                Ok(value) => io_poll_value(Poll::Ready, call_ok(), IoReady::empty(), value),
                Err(error) => io_poll_error(Poll::Ready, error, IoReady::empty()),
            }
        }
        IoOperationKind::Write => {
            let buffer =
                unsafe { std::slice::from_raw_parts(request.buffer(), request.buffer_len()) };
            match pipe.try_write(buffer) {
                Ok(value) => io_poll_value(Poll::Ready, call_ok(), IoReady::empty(), value),
                Err(error) => io_poll_error(Poll::Ready, error, IoReady::empty()),
            }
        }
        IoOperationKind::Disconnect => match pipe.disconnect() {
            Ok(()) => io_poll(Poll::Ready, call_ok(), IoReady::empty()),
            Err(error) => io_poll_error(Poll::Ready, error, IoReady::empty()),
        },
        IoOperationKind::Connect => {
            let mut future = registration.connect.lock().unwrap();
            if future.is_none() {
                *future = Some(connect_operation(registration.io.clone()));
            }
            let result = poll_completion_now(future.as_mut().unwrap().as_mut());
            if result.state != Poll::Pending {
                *future = None;
            }
            result
        }
    }
}

#[cfg(unix)]
unsafe extern "C" fn try_operate(_: *mut std::ffi::c_void, _: IoRequest) -> IoPoll {
    io_poll_error(
        Poll::Ready,
        io::Error::new(
            io::ErrorKind::Unsupported,
            "Unix I/O operations are guest-owned",
        ),
        IoReady::empty(),
    )
}

#[cfg(not(any(unix, windows)))]
#[expect(dead_code)]
unsafe extern "C" fn try_operate(_: *mut std::ffi::c_void, _: IoRequest) -> IoPoll {
    io_poll(Poll::Ready, call_ok(), IoReady::SHUTDOWN)
}

#[cfg(unix)]
unsafe extern "C" fn try_ready(data: *mut std::ffi::c_void, interest: IoInterest) -> IoPoll {
    let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
    registration
        .with(|registration| match host_interest(interest) {
            Ok(interest) => match registration.io.try_io(interest, |_| Ok(())) {
                Ok(()) => io_poll(Poll::Ready, call_ok(), interest_ready(interest)),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    io_poll(Poll::Pending, call_ok(), IoReady::empty())
                }
                Err(error) => io_poll_error(Poll::Ready, error, IoReady::SHUTDOWN),
            },
            Err(error) => io_poll_error(Poll::Ready, error, IoReady::empty()),
        })
        .unwrap_or_else(|error| {
            io_poll_error(Poll::Ready, io::Error::other(error), IoReady::SHUTDOWN)
        })
}

#[cfg(windows)]
unsafe extern "C" fn try_ready(data: *mut std::ffi::c_void, interest: IoInterest) -> IoPoll {
    let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
    registration
        .with(|registration| match host_interest(interest) {
            Ok(interest) => {
                let result = registration.io.try_io(interest, || Ok(()));
                match result {
                    Ok(()) => io_poll(Poll::Ready, call_ok(), interest_ready(interest)),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        io_poll(Poll::Pending, call_ok(), IoReady::empty())
                    }
                    Err(error) => io_poll_error(Poll::Ready, error, IoReady::SHUTDOWN),
                }
            }
            Err(error) => io_poll_error(Poll::Ready, error, IoReady::empty()),
        })
        .unwrap_or_else(|error| {
            io_poll_error(Poll::Ready, io::Error::other(error), IoReady::SHUTDOWN)
        })
}

#[cfg(not(any(unix, windows)))]
#[expect(dead_code)]
unsafe extern "C" fn try_ready(_: *mut std::ffi::c_void, _: IoInterest) -> IoPoll {
    io_poll(Poll::Ready, call_ok(), IoReady::SHUTDOWN)
}

#[cfg(any(unix, windows))]
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
    let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
    let _ = registration.with(|registration| {
        for interest in clear_interests(ready) {
            let _ = registration
                .io
                .try_io(interest, |_| Err::<(), _>(io::ErrorKind::WouldBlock.into()));
        }
    });
}

#[cfg(windows)]
unsafe extern "C" fn clear(data: *mut std::ffi::c_void, ready: IoReady) {
    let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
    let _ = registration.with(|registration| {
        for interest in clear_interests(ready) {
            let _ = registration
                .io
                .try_io(interest, || Err::<(), _>(io::ErrorKind::WouldBlock.into()));
        }
    });
}

#[cfg(not(any(unix, windows)))]
#[expect(dead_code)]
unsafe extern "C" fn clear(_: *mut std::ffi::c_void, _: IoReady) {}

#[cfg(any(unix, windows))]
unsafe extern "C" fn release_registration(data: *mut std::ffi::c_void) {
    unsafe { Arc::from_raw(data.cast::<HostResource<Registration>>()) }.release();
}

#[cfg(not(any(unix, windows)))]
#[expect(dead_code)]
unsafe extern "C" fn release_registration(_: *mut std::ffi::c_void) {}

#[cfg(any(unix, windows))]
unsafe extern "C" fn release_operation(data: *mut std::ffi::c_void) {
    unsafe { Arc::from_raw(data.cast::<HostResource<Operation>>()) }.release();
}

#[cfg(any(unix, windows))]
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

fn io_poll(state: Poll, call: CallResult, ready: IoReady) -> IoPoll {
    io_poll_value(state, call, ready, 0)
}

fn io_poll_value(state: Poll, call: CallResult, ready: IoReady, value: usize) -> IoPoll {
    IoPoll {
        state,
        call,
        error: IoError::none(),
        ready,
        value,
    }
}

fn io_poll_error(state: Poll, error: io::Error, ready: IoReady) -> IoPoll {
    IoPoll {
        state,
        error: IoError::from_error(&error),
        call: call_error(error),
        ready,
        value: 0,
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
