use super::*;
use crate::io::ready::Ready;
use crate::runtime::io::telekio::Source as TelekioSource;

impl Registration {
    #[track_caller]
    pub(crate) fn new_with_interest_and_handle(
        io: &mut (impl mio::event::Source + TelekioSource),
        interest: Interest,
        handle: scheduler::Handle,
    ) -> io::Result<Self> {
        let telekio = handle
            .host()
            .register_io(io.telekio_resource(), telekio_interest(interest))?;
        let shared = Arc::new(ScheduledIo::default());
        shared.install(telekio);
        Ok(Self { handle, shared })
    }

    pub(crate) fn deregister(&mut self, _: &mut impl mio::event::Source) -> io::Result<()> {
        self.shared.close_telekio();
        Ok(())
    }

    pub(crate) fn poll_read_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<ReadyEvent>> {
        self.poll_host_ready(cx, Interest::READABLE)
    }

    pub(crate) fn poll_write_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<ReadyEvent>> {
        self.poll_host_ready(cx, Interest::WRITABLE)
    }

    pub(super) fn poll_ready(
        &self,
        cx: &mut Context<'_>,
        direction: Direction,
    ) -> Poll<io::Result<ReadyEvent>> {
        self.poll_host_ready(
            cx,
            match direction {
                Direction::Read => Interest::READABLE,
                Direction::Write => Interest::WRITABLE,
            },
        )
    }

    fn poll_host_ready(
        &self,
        cx: &mut Context<'_>,
        interest: Interest,
    ) -> Poll<io::Result<ReadyEvent>> {
        let waker = ::telekio::Waker::from_ref(cx.waker());
        let result = self
            .shared
            .poll_telekio(telekio_interest(interest), &waker);
        match result.state {
            ::telekio::Poll::Pending => {
                unsafe { result.call.payload.release() };
                Poll::Pending
            }
            ::telekio::Poll::Ready => {
                result.call.into_io_result()?;
                if result.ready.contains(::telekio::IoReady::SHUTDOWN) {
                    Poll::Ready(Err(gone()))
                } else {
                    Poll::Ready(Ok(ready_event(result.ready)))
                }
            }
            ::telekio::Poll::Panicked => {
                result.call.into_io_result()?;
                unreachable!()
            }
        }
    }

    pub(crate) async fn readiness(&self, interest: Interest) -> io::Result<ReadyEvent> {
        let ready = self
            .shared
            .ready_telekio(telekio_interest(interest))
            .await?;
        if ready.contains(::telekio::IoReady::SHUTDOWN) {
            Err(gone())
        } else {
            Ok(ready_event(ready))
        }
    }

    pub(crate) fn try_io<R>(
        &self,
        interest: Interest,
        f: impl FnOnce() -> io::Result<R>,
    ) -> io::Result<R> {
        let result = self
            .shared
            .try_ready_telekio(telekio_interest(interest));
        match result.state {
            ::telekio::Poll::Pending => {
                unsafe { result.call.payload.release() };
                Err(io::ErrorKind::WouldBlock.into())
            }
            ::telekio::Poll::Ready => {
                result.call.into_io_result()?;
                match f() {
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        self.shared.clear_telekio(result.ready);
                        Err(error)
                    }
                    result => result,
                }
            }
            ::telekio::Poll::Panicked => {
                result.call.into_io_result()?;
                unreachable!()
            }
        }
    }

    pub(crate) fn clear_readiness(&self, event: ReadyEvent) {
        self.shared.clear_telekio(::telekio::IoReady::from_bits(
            event.ready.as_usize() as u8,
        ));
    }
}

fn ready_event(ready: ::telekio::IoReady) -> ReadyEvent {
    ReadyEvent {
        tick: 0,
        ready: Ready::from_usize(ready.bits() as usize),
        is_shutdown: false,
    }
}

fn telekio_interest(interest: Interest) -> ::telekio::IoInterest {
    let mut result = ::telekio::IoInterest::empty();
    if interest.is_readable() {
        result |= ::telekio::IoInterest::READABLE;
    }
    if interest.is_writable() {
        result |= ::telekio::IoInterest::WRITABLE;
    }
    if interest.is_error() {
        result |= ::telekio::IoInterest::ERROR;
    }
    #[cfg(any(target_os = "android", target_os = "linux"))]
    if interest.is_priority() {
        result |= ::telekio::IoInterest::PRIORITY;
    }
    result
}
