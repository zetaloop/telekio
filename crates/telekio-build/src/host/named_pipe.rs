use super::NamedPipeServer;
use crate::io::{Interest, Ready};
use std::{
    io,
    task::{Context, Poll},
};

impl NamedPipeServer {
    #[doc(hidden)]
    pub fn poll_telekio_ready(
        &self,
        context: &mut Context<'_>,
        interest: Interest,
    ) -> Poll<io::Result<(u8, Ready, bool)>> {
        self.io.poll_telekio_ready(context, interest)
    }

    #[doc(hidden)]
    pub async fn telekio_ready(&self, interest: Interest) -> io::Result<(u8, Ready, bool)> {
        self.io.telekio_ready(interest).await
    }

    #[doc(hidden)]
    pub fn try_telekio_ready(&self, interest: Interest) -> (u8, Ready, bool) {
        self.io.try_telekio_ready(interest)
    }

    #[doc(hidden)]
    pub fn clear_telekio_ready(&self, tick: u8, ready: Ready) {
        self.io.clear_telekio_ready(tick, ready);
    }
}
