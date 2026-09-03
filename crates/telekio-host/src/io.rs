use std::{
    io,
    panic::{AssertUnwindSafe, catch_unwind},
};

use std::sync::Arc;
#[cfg(windows)]
use std::sync::Mutex;
#[cfg(any(unix, windows))]
use std::{future::Future, pin::Pin};

use telekio::{
    CallResult, Callback, IoCallResult, IoDriverRegistration, IoDriverResult, IoError, IoInterest,
    IoOperationResult, IoPoll, IoReady, IoRegistration, IoRequest, IoResource, IoResult,
    OwnedBytes, Poll, Status, Waker,
};

#[cfg(any(unix, windows))]
use telekio::IoKind;
#[cfg(windows)]
use telekio::IoOperationKind;

#[cfg(target_os = "linux")]
use super::{CallbackOwner, host_callback};
use super::{HandleContext, HostResource};

#[cfg(any(unix, windows))]
type OperationFuture = Pin<Box<dyn Future<Output = io::Result<HostReady>> + Send>>;

#[cfg(any(unix, windows))]
struct HostReady {
    tick: u8,
    ready: IoReady,
}

#[cfg(not(any(unix, windows)))]
struct Registration;

#[cfg(unix)]
struct RawIo(std::os::fd::RawFd);

#[cfg(unix)]
impl std::os::fd::AsRawFd for RawIo {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.0
    }
}

#[cfg(unix)]
#[derive(Clone)]
enum UnixIo {
    Fd(Arc<tokio::io::unix::AsyncFd<RawIo>>),
    #[cfg(target_os = "freebsd")]
    Aio(Arc<tokio::runtime::telekio::TelekioAio>),
}

#[cfg(unix)]
struct Registration {
    io: UnixIo,
}

#[cfg(unix)]
impl UnixIo {
    fn poll_ready(
        &self,
        context: &mut std::task::Context<'_>,
        interest: IoInterest,
    ) -> std::task::Poll<io::Result<(u8, tokio::io::Ready, bool)>> {
        match self {
            Self::Fd(io) => match host_interest(interest) {
                Ok(interest) => io.poll_telekio_ready(context, interest),
                Err(error) => std::task::Poll::Ready(Err(error)),
            },
            #[cfg(target_os = "freebsd")]
            Self::Aio(io) => io.poll_ready(context),
        }
    }

    async fn ready(&self, interest: IoInterest) -> io::Result<(u8, tokio::io::Ready, bool)> {
        match self {
            Self::Fd(io) => io.telekio_ready(host_interest(interest)?).await,
            #[cfg(target_os = "freebsd")]
            Self::Aio(io) => io.ready().await,
        }
    }

    fn try_ready(
        &self,
        interest: IoInterest,
    ) -> std::task::Poll<io::Result<(u8, tokio::io::Ready, bool)>> {
        match self {
            Self::Fd(io) => {
                std::task::Poll::Ready(Ok(io.try_telekio_ready(host_interest(interest)?)))
            }
            #[cfg(target_os = "freebsd")]
            Self::Aio(io) => io.try_ready(),
        }
    }

    fn clear_ready(&self, tick: u8, ready: tokio::io::Ready) {
        match self {
            Self::Fd(io) => io.clear_telekio_ready(tick, ready),
            #[cfg(target_os = "freebsd")]
            Self::Aio(io) => io.clear_ready(tick, ready),
        }
    }
}

#[cfg(windows)]
#[derive(Clone)]
enum WindowsIo {
    Socket(Arc<tokio::net::telekio::Socket>),
    Pipe(Arc<tokio::net::windows::named_pipe::NamedPipeServer>),
}

#[cfg(windows)]
impl WindowsIo {
    fn poll_telekio_ready(
        &self,
        context: &mut std::task::Context<'_>,
        interest: tokio::io::Interest,
    ) -> std::task::Poll<io::Result<(u8, tokio::io::Ready, bool)>> {
        match self {
            Self::Socket(io) => io.poll_ready(context, interest),
            Self::Pipe(io) => io.poll_telekio_ready(context, interest),
        }
    }

    async fn telekio_ready(
        &self,
        interest: tokio::io::Interest,
    ) -> io::Result<(u8, tokio::io::Ready, bool)> {
        match self {
            Self::Socket(io) => io.ready(interest).await,
            Self::Pipe(io) => io.telekio_ready(interest).await,
        }
    }

    fn try_telekio_ready(&self, interest: tokio::io::Interest) -> (u8, tokio::io::Ready, bool) {
        match self {
            Self::Socket(io) => io.try_ready(interest),
            Self::Pipe(io) => io.try_telekio_ready(interest),
        }
    }

    fn clear_telekio_ready(&self, tick: u8, ready: tokio::io::Ready) -> io::Result<()> {
        match self {
            Self::Socket(io) => io.clear_ready(tick, ready),
            Self::Pipe(io) => {
                io.clear_telekio_ready(tick, ready);
                Ok(())
            }
        }
    }
}

#[cfg(windows)]
struct Registration {
    io: WindowsIo,
    connect: Mutex<Option<OperationFuture>>,
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

#[cfg(unix)]
fn register_inner(
    context: &HandleContext,
    resource: IoResource,
    interest: IoInterest,
) -> io::Result<Registration> {
    #[cfg(target_os = "freebsd")]
    use std::os::fd::AsRawFd;
    use std::os::fd::RawFd;

    let io = match resource.kind() {
        IoKind::Fd => {
            let raw = resource.raw() as u32 as RawFd;
            if raw < 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid file descriptor",
                ));
            }
            let _guard = context.handle.enter();
            UnixIo::Fd(Arc::new(tokio::io::unix::AsyncFd::with_interest(
                RawIo(raw),
                host_interest(interest)?,
            )?))
        }
        #[cfg(target_os = "freebsd")]
        IoKind::Aio => {
            if !interest.contains(IoInterest::AIO) && !interest.contains(IoInterest::LIO) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "AIO interest is empty",
                ));
            }
            let _guard = context.handle.enter();
            UnixIo::Aio(Arc::new(context.handle.telekio_register_aio(
                Arc::new(move |kqueue, token| {
                    resource
                        .configure_aio(kqueue.as_raw_fd(), token)
                        .into_io_result()
                }),
                interest.contains(IoInterest::LIO),
            )?))
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a Unix I/O resource",
            ));
        }
    };
    Ok(Registration { io })
}

#[cfg(windows)]
fn register_inner(
    context: &HandleContext,
    resource: IoResource,
    interest: IoInterest,
) -> io::Result<Registration> {
    use std::os::windows::io::{BorrowedHandle, IntoRawHandle, RawHandle, RawSocket};

    let io = match resource.kind() {
        IoKind::Socket => {
            let mut interest = interest;
            interest |= IoInterest::ERROR;
            let _guard = context.handle.enter();
            WindowsIo::Socket(Arc::new(unsafe {
                tokio::net::telekio::Socket::from_raw_socket(
                    resource.raw() as RawSocket,
                    host_interest(interest)?,
                )?
            }))
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
        IoKind::Fd | IoKind::Aio => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a Windows socket or handle",
            ));
        }
    };
    Ok(Registration {
        io,
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
    io_callback(|| {
        let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
        registration.update_waker(unsafe { &*waker });
        let waker = unsafe { (*waker).clone_rust_waker() };
        let mut context = std::task::Context::from_waker(&waker);
        registration
            .with(|registration| registration.io.poll_ready(&mut context, interest))
            .map(ready_poll)
            .unwrap_or_else(|error| {
                io_poll_error(Poll::Ready, io::Error::other(error), IoReady::SHUTDOWN)
            })
    })
}

#[cfg(windows)]
unsafe extern "C" fn poll_registration(
    data: *mut std::ffi::c_void,
    interest: IoInterest,
    waker: *const Waker,
) -> IoPoll {
    io_callback(|| {
        let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
        registration.update_waker(unsafe { &*waker });
        registration
            .with(|registration| poll_windows_registration(registration, interest, waker))
            .unwrap_or_else(|error| {
                io_poll_error(Poll::Ready, io::Error::other(error), IoReady::SHUTDOWN)
            })
    })
}

#[cfg(windows)]
fn poll_windows_registration(
    registration: &Registration,
    interest: IoInterest,
    waker: *const Waker,
) -> IoPoll {
    let waker = unsafe { (*waker).clone_rust_waker() };
    let mut context = std::task::Context::from_waker(&waker);
    let result = match host_interest(interest) {
        Ok(interest) => registration.io.poll_telekio_ready(&mut context, interest),
        Err(error) => std::task::Poll::Ready(Err(error)),
    };
    ready_poll(result)
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
unsafe extern "C" fn ready(data: *mut std::ffi::c_void, interest: IoInterest) -> IoOperationResult {
    match catch_unwind(AssertUnwindSafe(|| {
        let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
        let owner = Arc::clone(&registration.owner);
        let future =
            registration.with(|registration| operation_ready(registration.io.clone(), interest))?;
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

#[cfg(not(any(unix, windows)))]
#[expect(dead_code)]
unsafe extern "C" fn ready(_: *mut std::ffi::c_void, _: IoInterest) -> IoOperationResult {
    IoOperationResult {
        call: CallResult::ok(),
        operation: telekio::IoOperation::empty(),
    }
}

#[cfg(any(unix, windows))]
unsafe extern "C" fn poll_operation(data: *mut std::ffi::c_void, waker: *const Waker) -> IoPoll {
    io_callback(|| {
        let operation = unsafe { &*data.cast::<HostResource<Operation>>() };
        operation.update_waker(unsafe { &*waker });
        operation
            .with_mut(|operation| poll_completion(operation.future.as_mut(), waker))
            .unwrap_or_else(|error| {
                io_poll_error(Poll::Ready, io::Error::other(error), IoReady::SHUTDOWN)
            })
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

#[cfg(windows)]
fn poll_completion_now(
    future: Pin<&mut (dyn Future<Output = io::Result<HostReady>> + Send)>,
) -> IoPoll {
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
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

#[cfg(windows)]
fn operation_error(error: io::Error) -> OperationFuture {
    Box::pin(async move { Err(error) })
}

#[cfg(unix)]
fn operation_ready(io: UnixIo, interest: IoInterest) -> OperationFuture {
    Box::pin(async move { io.ready(interest).await.map(host_ready) })
}

#[cfg(windows)]
fn operation_ready(io: WindowsIo, interest: IoInterest) -> OperationFuture {
    match host_interest(interest) {
        Ok(interest) => Box::pin(async move { io.telekio_ready(interest).await.map(host_ready) }),
        Err(error) => operation_error(error),
    }
}

#[cfg(windows)]
fn connect_operation(io: WindowsIo) -> OperationFuture {
    match io {
        WindowsIo::Pipe(pipe) => Box::pin(async move {
            pipe.connect().await.map(|_| HostReady {
                tick: 0,
                ready: IoReady::empty(),
            })
        }),
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
    io_callback(|| {
        let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
        registration
            .with(|registration| try_operate_inner(registration, request))
            .unwrap_or_else(|error| {
                io_poll_error(Poll::Ready, io::Error::other(error), IoReady::SHUTDOWN)
            })
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
    io_callback(|| {
        let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
        registration
            .with(|registration| ready_poll(registration.io.try_ready(interest)))
            .unwrap_or_else(|error| {
                io_poll_error(Poll::Ready, io::Error::other(error), IoReady::SHUTDOWN)
            })
    })
}

#[cfg(windows)]
unsafe extern "C" fn try_ready(data: *mut std::ffi::c_void, interest: IoInterest) -> IoPoll {
    io_callback(|| {
        let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
        registration
            .with(|registration| match host_interest(interest) {
                Ok(interest) => ready_now(host_ready(registration.io.try_telekio_ready(interest))),
                Err(error) => io_poll_error(Poll::Ready, error, IoReady::empty()),
            })
            .unwrap_or_else(|error| {
                io_poll_error(Poll::Ready, io::Error::other(error), IoReady::SHUTDOWN)
            })
    })
}

#[cfg(not(any(unix, windows)))]
#[expect(dead_code)]
unsafe extern "C" fn try_ready(_: *mut std::ffi::c_void, _: IoInterest) -> IoPoll {
    io_poll(Poll::Ready, call_ok(), IoReady::SHUTDOWN)
}

#[cfg(any(unix, windows))]
fn ready_now(ready: HostReady) -> IoPoll {
    if ready.ready.bits() == 0 {
        io_poll(Poll::Pending, call_ok(), IoReady::empty())
    } else {
        io_poll_event(Poll::Ready, call_ok(), ready.tick, ready.ready)
    }
}

#[cfg(unix)]
unsafe extern "C" fn clear(data: *mut std::ffi::c_void, tick: u8, ready: IoReady) -> IoCallResult {
    io_call(|| {
        let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
        let _ = registration.with(|registration| {
            registration.io.clear_ready(tick, tokio_ready(ready));
        });
        Ok(())
    })
}

#[cfg(windows)]
unsafe extern "C" fn clear(data: *mut std::ffi::c_void, tick: u8, ready: IoReady) -> IoCallResult {
    io_call(|| {
        let registration = unsafe { &*data.cast::<HostResource<Registration>>() };
        if let Ok(result) = registration.with(|registration| {
            registration
                .io
                .clear_telekio_ready(tick, tokio_ready(ready))
        }) {
            result?;
        }
        Ok(())
    })
}

#[cfg(not(any(unix, windows)))]
#[expect(dead_code)]
unsafe extern "C" fn clear(_: *mut std::ffi::c_void, _: u8, _: IoReady) -> IoCallResult {
    IoCallResult {
        call: CallResult::ok(),
        error: IoError::none(),
    }
}

#[cfg(any(unix, windows))]
unsafe extern "C" fn release_registration(data: *mut std::ffi::c_void) -> CallResult {
    super::host_callback(|| {
        unsafe { &*data.cast::<HostResource<Registration>>() }.release();
    })
}

#[cfg(not(any(unix, windows)))]
#[expect(dead_code)]
unsafe extern "C" fn release_registration(_: *mut std::ffi::c_void) -> CallResult {
    CallResult::ok()
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
