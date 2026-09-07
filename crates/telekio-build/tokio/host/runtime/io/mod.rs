#[cfg(target_os = "linux")]
pub(crate) use super::driver::telekio::Registration;

#[cfg(all(target_os = "freebsd", feature = "net"))]
mod aio {
    use super::super::{Direction, Handle, ReadyEvent, ScheduledIo};
    use crate::io::{Interest, Ready};
    use std::{
        io,
        os::fd::{AsFd, BorrowedFd},
        sync::Arc,
        task::{Context, Poll},
    };

    type Configure = Arc<dyn for<'a> Fn(BorrowedFd<'a>, usize) -> io::Result<()> + Send + Sync>;

    pub(crate) struct Aio {
        io: Arc<ScheduledIo>,
        source: Source,
    }

    struct Source {
        configure: Option<Configure>,
    }

    impl mio::event::Source for Source {
        fn register(
            &mut self,
            registry: &mio::Registry,
            token: mio::Token,
            interest: mio::Interest,
        ) -> io::Result<()> {
            assert!(interest.is_aio() || interest.is_lio());
            self.configure
                .take()
                .ok_or_else(|| io::Error::other("AIO source is already registered"))?(
                registry.as_fd(),
                usize::from(token),
            )
        }

        fn reregister(
            &mut self,
            _: &mio::Registry,
            _: mio::Token,
            _: mio::Interest,
        ) -> io::Result<()> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "AIO source cannot be reregistered",
            ))
        }

        fn deregister(&mut self, _: &mio::Registry) -> io::Result<()> {
            Ok(())
        }
    }

    impl Handle {
        pub(crate) fn telekio_register_aio(
            &self,
            configure: Configure,
            lio: bool,
        ) -> io::Result<Aio> {
            let mut source = Source {
                configure: Some(configure),
            };
            let interest = if lio { Interest::LIO } else { Interest::AIO };
            let io = self.add_source(&mut source, interest)?;
            Ok(Aio { io, source })
        }

        pub(crate) fn telekio_deregister_aio(&self, mut aio: Aio) {
            aio.io.clear_wakers();
            _ = self.deregister_source(&aio.io, &mut aio.source);
        }
    }

    impl Aio {
        pub(crate) fn poll_ready(
            &self,
            context: &mut Context<'_>,
        ) -> Poll<io::Result<(u8, Ready, bool)>> {
            self.io
                .poll_readiness(context, Direction::Read)
                .map(|event| Ok((event.tick, event.ready, event.is_shutdown)))
        }

        pub(crate) async fn ready(&self) -> io::Result<(u8, Ready, bool)> {
            std::future::poll_fn(|context| self.poll_ready(context)).await
        }

        pub(crate) fn try_ready(&self) -> Poll<io::Result<(u8, Ready, bool)>> {
            self.poll_ready(&mut Context::from_waker(std::task::Waker::noop()))
        }

        pub(crate) fn clear_ready(&self, tick: u8, ready: Ready) {
            self.io.clear_readiness(ReadyEvent {
                tick,
                ready,
                is_shutdown: false,
            });
        }
    }
}

#[cfg(all(target_os = "freebsd", feature = "net"))]
pub(crate) use aio::Aio;
