use super::*;
#[cfg(feature = "rt")]
use crate::io::ready::Ready;
use crate::runtime::io::telekio::Source as TelekioSource;

impl Registration {
    #[track_caller]
    pub(crate) fn new_with_interest_and_handle(
        io: &mut (impl mio::event::Source + TelekioSource),
        interest: Interest,
        handle: scheduler::Handle,
    ) -> io::Result<Self> {
        #[cfg(not(feature = "rt"))]
        return Self::register_local(io, interest, handle);

        #[cfg(feature = "rt")]
        {
            let telekio = handle
                .connection()
                .handle
                .register_io(io.telekio_resource(), telekio_interest(interest))?;
            let shared = Arc::new(ScheduledIo::default());
            shared.install(telekio);
            Ok(Self { handle, shared })
        }
    }

    pub(crate) fn deregister(&mut self, io: &mut impl mio::event::Source) -> io::Result<()> {
        #[cfg(not(feature = "rt"))]
        return self.deregister_local(io);

        #[cfg(feature = "rt")]
        {
            let _ = io;
            self.shared.close_telekio();
            Ok(())
        }
    }

    pub(crate) fn poll_read_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<ReadyEvent>> {
        #[cfg(not(feature = "rt"))]
        return self.poll_local_read_ready(cx);

        #[cfg(feature = "rt")]
        self.poll_host_ready(cx, Interest::READABLE)
    }

    pub(crate) fn poll_write_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<ReadyEvent>> {
        #[cfg(not(feature = "rt"))]
        return self.poll_local_write_ready(cx);

        #[cfg(feature = "rt")]
        self.poll_host_ready(cx, Interest::WRITABLE)
    }

    pub(super) fn poll_ready(
        &self,
        cx: &mut Context<'_>,
        direction: Direction,
    ) -> Poll<io::Result<ReadyEvent>> {
        #[cfg(not(feature = "rt"))]
        return self.poll_local_ready(cx, direction);

        #[cfg(feature = "rt")]
        self.poll_host_ready(
            cx,
            match direction {
                Direction::Read => Interest::READABLE,
                Direction::Write => Interest::WRITABLE,
            },
        )
    }

    #[cfg(feature = "rt")]
    fn poll_host_ready(
        &self,
        cx: &mut Context<'_>,
        interest: Interest,
    ) -> Poll<io::Result<ReadyEvent>> {
        if let Some(error) = self.shared.take_telekio_error() {
            return Poll::Ready(Err(error));
        }
        let waker = unsafe { ::telekio_abi::Waker::from_ref(cx.waker()) };
        let result = self.shared.poll_telekio(telekio_interest(interest), &waker);
        match result.state {
            ::telekio_abi::Poll::Pending => {
                unsafe { result.call.payload.release() };
                Poll::Pending
            }
            ::telekio_abi::Poll::Ready => {
                result.call.into_io_result()?;
                if result.ready.contains(::telekio_abi::IoReady::SHUTDOWN) {
                    Poll::Ready(Err(gone()))
                } else {
                    Poll::Ready(Ok(ready_event(result.ready, result.tick)))
                }
            }
            ::telekio_abi::Poll::Panicked => {
                result.call.into_io_result()?;
                unreachable!()
            }
        }
    }

    pub(crate) async fn readiness(&self, interest: Interest) -> io::Result<ReadyEvent> {
        #[cfg(not(feature = "rt"))]
        return self.local_readiness(interest).await;

        #[cfg(feature = "rt")]
        {
            if let Some(error) = self.shared.take_telekio_error() {
                return Err(error);
            }
            let event = self
                .shared
                .ready_telekio(telekio_interest(interest))
                .await?;
            if event.ready.contains(::telekio_abi::IoReady::SHUTDOWN) {
                Err(gone())
            } else {
                Ok(ready_event(event.ready, event.tick))
            }
        }
    }

    #[cfg(windows)]
    pub(crate) fn try_operate(&self, request: ::telekio_abi::IoRequest) -> ::telekio_abi::IoPoll {
        self.shared.try_operate_telekio(request)
    }

    pub(crate) fn try_io<R>(
        &self,
        interest: Interest,
        f: impl FnOnce() -> io::Result<R>,
    ) -> io::Result<R> {
        #[cfg(not(feature = "rt"))]
        return self.local_try_io(interest, f);

        #[cfg(feature = "rt")]
        {
            if let Some(error) = self.shared.take_telekio_error() {
                return Err(error);
            }
            let result = self.shared.try_ready_telekio(telekio_interest(interest));
            match result.state {
                ::telekio_abi::Poll::Pending => {
                    unsafe { result.call.payload.release() };
                    Err(io::ErrorKind::WouldBlock.into())
                }
                ::telekio_abi::Poll::Ready => {
                    result.call.into_io_result()?;
                    match f() {
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            self.shared
                                .clear_telekio_result(result.tick, result.ready)?;
                            Err(error)
                        }
                        result => result,
                    }
                }
                ::telekio_abi::Poll::Panicked => {
                    result.call.into_io_result()?;
                    unreachable!()
                }
            }
        }
    }

    pub(crate) fn clear_readiness(&self, event: ReadyEvent) {
        #[cfg(not(feature = "rt"))]
        return self.clear_local_readiness(event);

        #[cfg(feature = "rt")]
        self.shared.clear_telekio(
            event.tick,
            ::telekio_abi::IoReady::from_bits(event.ready.as_usize() as u8),
        );
    }
}

#[cfg(feature = "rt")]
fn ready_event(ready: ::telekio_abi::IoReady, tick: u8) -> ReadyEvent {
    ReadyEvent {
        tick,
        ready: Ready::from_usize(ready.bits() as usize),
        is_shutdown: false,
    }
}

#[cfg(feature = "rt")]
fn telekio_interest(interest: Interest) -> ::telekio_abi::IoInterest {
    let mut result = ::telekio_abi::IoInterest::empty();
    if interest.is_readable() {
        result |= ::telekio_abi::IoInterest::READABLE;
    }
    if interest.is_writable() {
        result |= ::telekio_abi::IoInterest::WRITABLE;
    }
    if interest.is_error() {
        result |= ::telekio_abi::IoInterest::ERROR;
    }
    #[cfg(target_os = "freebsd")]
    if interest.is_aio() {
        result |= ::telekio_abi::IoInterest::AIO;
    }
    #[cfg(target_os = "freebsd")]
    if interest.is_lio() {
        result |= ::telekio_abi::IoInterest::LIO;
    }
    #[cfg(any(target_os = "android", target_os = "linux"))]
    if interest.is_priority() {
        result |= ::telekio_abi::IoInterest::PRIORITY;
    }
    result
}
