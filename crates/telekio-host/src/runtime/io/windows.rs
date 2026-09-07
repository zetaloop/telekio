use super::*;
use std::sync::Mutex;
use telekio::IoOperationKind;

#[derive(Clone)]
enum WindowsIo {
    Socket(Arc<tokio::net::telekio::Socket>),
    Pipe(Arc<tokio::net::windows::named_pipe::NamedPipeServer>),
}

pub(super) struct Registration {
    io: WindowsIo,
    connect: Mutex<Option<OperationFuture>>,
}

pub(super) fn register_inner(
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

impl Registration {
    pub(super) fn poll_ready(
        &self,
        context: &mut std::task::Context<'_>,
        interest: IoInterest,
    ) -> std::task::Poll<io::Result<(u8, tokio::runtime::telekio::Ready, bool)>> {
        match (host_interest(interest), &self.io) {
            (Ok(interest), WindowsIo::Socket(io)) => io.poll_ready(context, interest),
            (Ok(interest), WindowsIo::Pipe(io)) => io.poll_telekio_ready(context, interest),
            (Err(error), _) => std::task::Poll::Ready(Err(error)),
        }
    }

    pub(super) fn ready(&self, interest: IoInterest) -> OperationFuture {
        let io = self.io.clone();
        match (host_interest(interest), io) {
            (Ok(interest), WindowsIo::Socket(io)) => {
                Box::pin(async move { io.ready(interest).await.map(host_ready) })
            }
            (Ok(interest), WindowsIo::Pipe(io)) => {
                Box::pin(async move { io.telekio_ready(interest).await.map(host_ready) })
            }
            (Err(error), _) => operation_error(error),
        }
    }

    pub(super) fn try_operate(&self, request: IoRequest) -> IoPoll {
        let WindowsIo::Pipe(pipe) = &self.io else {
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
                let buffer = unsafe {
                    std::slice::from_raw_parts_mut(request.buffer(), request.buffer_len())
                };
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
                let mut future = self.connect.lock().unwrap();
                if future.is_none() {
                    *future = Some(connect_operation(self.io.clone()));
                }
                let result = poll_completion_now(future.as_mut().unwrap().as_mut());
                if result.state != Poll::Pending {
                    *future = None;
                }
                result
            }
        }
    }

    pub(super) fn try_ready(&self, interest: IoInterest) -> IoPoll {
        match (host_interest(interest), &self.io) {
            (Ok(interest), WindowsIo::Socket(io)) => ready_now(host_ready(io.try_ready(interest))),
            (Ok(interest), WindowsIo::Pipe(io)) => {
                ready_now(host_ready(io.try_telekio_ready(interest)))
            }
            (Err(error), _) => io_poll_error(Poll::Ready, error, IoReady::empty()),
        }
    }

    pub(super) fn clear(&self, tick: u8, ready: IoReady) -> io::Result<()> {
        match &self.io {
            WindowsIo::Socket(io) => io.clear_ready(tick, tokio_ready(ready)),
            WindowsIo::Pipe(io) => {
                io.clear_telekio_ready(tick, tokio_ready(ready));
                Ok(())
            }
        }
    }
}

fn operation_error(error: io::Error) -> OperationFuture {
    Box::pin(async move { Err(error) })
}

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

fn pipe_error(error: io::Error) -> IoPoll {
    io_poll_error(Poll::Ready, error, IoReady::SHUTDOWN)
}

fn poll_completion_now(
    future: Pin<&mut (dyn Future<Output = io::Result<HostReady>> + Send)>,
) -> IoPoll {
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    completion_poll(future.poll(&mut context))
}
