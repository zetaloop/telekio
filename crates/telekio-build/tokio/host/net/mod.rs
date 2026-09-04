use crate::io::{Interest, PollEvented, Ready};
use mio::event::Source;
use std::{
    io,
    mem::ManuallyDrop,
    os::windows::io::{AsRawSocket, FromRawSocket, RawSocket},
    task::{Context, Poll},
};

#[derive(Debug)]
#[doc(hidden)]
pub struct Socket(PollEvented<BorrowedSocket>);

#[derive(Debug)]
struct BorrowedSocket(ManuallyDrop<mio::net::UdpSocket>);

impl Socket {
    /// # Safety
    ///
    /// `socket` must remain valid until this registration is dropped.
    pub unsafe fn from_raw_socket(socket: RawSocket, interest: Interest) -> io::Result<Self> {
        let socket = unsafe { std::net::UdpSocket::from_raw_socket(socket) };
        let socket = mio::net::UdpSocket::from_std(socket);
        PollEvented::new_with_interest(BorrowedSocket(ManuallyDrop::new(socket)), interest)
            .map(Self)
    }

    pub fn poll_ready(
        &self,
        context: &mut Context<'_>,
        interest: Interest,
    ) -> Poll<io::Result<(u8, Ready, bool)>> {
        self.0.poll_telekio_ready(context, interest)
    }

    pub async fn ready(&self, interest: Interest) -> io::Result<(u8, Ready, bool)> {
        self.0.telekio_ready(interest).await
    }

    pub fn try_ready(&self, interest: Interest) -> (u8, Ready, bool) {
        self.0.try_telekio_ready(interest)
    }

    pub fn clear_ready(&self, tick: u8, ready: Ready) -> io::Result<()> {
        self.0.rearm()?;
        self.0.clear_telekio_ready(tick, ready);
        Ok(())
    }
}

impl BorrowedSocket {
    fn rearm(&self) -> io::Result<()> {
        match self
            .0
            .try_io(|| Err::<(), _>(io::ErrorKind::WouldBlock.into()))
        {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(()),
            Err(error) => Err(error),
            Ok(()) => unreachable!(),
        }
    }
}

impl AsRawSocket for BorrowedSocket {
    fn as_raw_socket(&self) -> RawSocket {
        self.0.as_raw_socket()
    }
}

impl Source for BorrowedSocket {
    fn register(
        &mut self,
        registry: &mio::Registry,
        token: mio::Token,
        interests: mio::Interest,
    ) -> io::Result<()> {
        self.0.register(registry, token, interests)
    }

    fn reregister(
        &mut self,
        registry: &mio::Registry,
        token: mio::Token,
        interests: mio::Interest,
    ) -> io::Result<()> {
        self.0.reregister(registry, token, interests)
    }

    fn deregister(&mut self, registry: &mio::Registry) -> io::Result<()> {
        self.0.deregister(registry)
    }
}
