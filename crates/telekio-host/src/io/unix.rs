use super::*;
use std::os::fd::{AsRawFd, RawFd};

struct RawIo(RawFd);

impl AsRawFd for RawIo {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

#[derive(Clone)]
enum UnixIo {
    Fd(Arc<tokio::io::unix::AsyncFd<RawIo>>),
    #[cfg(target_os = "freebsd")]
    Aio(Arc<tokio::runtime::telekio::TelekioAio>),
}

pub(super) struct Registration {
    io: UnixIo,
}

pub(super) fn register_inner(
    context: &HandleContext,
    resource: IoResource,
    interest: IoInterest,
) -> io::Result<Registration> {
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

impl Registration {
    pub(super) fn poll_ready(
        &self,
        context: &mut std::task::Context<'_>,
        interest: IoInterest,
    ) -> std::task::Poll<io::Result<(u8, tokio::io::Ready, bool)>> {
        match &self.io {
            UnixIo::Fd(io) => match host_interest(interest) {
                Ok(interest) => io.poll_telekio_ready(context, interest),
                Err(error) => std::task::Poll::Ready(Err(error)),
            },
            #[cfg(target_os = "freebsd")]
            UnixIo::Aio(io) => io.poll_ready(context),
        }
    }

    pub(super) fn ready(&self, interest: IoInterest) -> OperationFuture {
        let io = self.io.clone();
        Box::pin(async move {
            match io {
                UnixIo::Fd(io) => io.telekio_ready(host_interest(interest)?).await,
                #[cfg(target_os = "freebsd")]
                UnixIo::Aio(io) => io.ready().await,
            }
            .map(host_ready)
        })
    }

    pub(super) fn try_operate(&self, _: IoRequest) -> IoPoll {
        io_poll_error(
            Poll::Ready,
            io::Error::new(
                io::ErrorKind::Unsupported,
                "Unix I/O operations are guest-owned",
            ),
            IoReady::empty(),
        )
    }

    pub(super) fn try_ready(&self, interest: IoInterest) -> IoPoll {
        ready_poll(match &self.io {
            UnixIo::Fd(io) => match host_interest(interest) {
                Ok(interest) => std::task::Poll::Ready(Ok(io.try_telekio_ready(interest))),
                Err(error) => std::task::Poll::Ready(Err(error)),
            },
            #[cfg(target_os = "freebsd")]
            UnixIo::Aio(io) => io.try_ready(),
        })
    }

    pub(super) fn clear(&self, tick: u8, ready: IoReady) -> io::Result<()> {
        match &self.io {
            UnixIo::Fd(io) => io.clear_telekio_ready(tick, tokio_ready(ready)),
            #[cfg(target_os = "freebsd")]
            UnixIo::Aio(io) => io.clear_ready(tick, tokio_ready(ready)),
        }
        Ok(())
    }
}
