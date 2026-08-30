use super::*;
use crate::io::{Interest, Ready};
use std::{
    io,
    task::{Context, Poll},
};

impl<E: Source> PollEvented<E> {
    pub(crate) fn poll_telekio_ready(
        &self,
        context: &mut Context<'_>,
        interest: Interest,
    ) -> Poll<io::Result<(u8, Ready, bool)>> {
        self.registration
            .poll_telekio_ready(context, interest.is_writable())
    }

    pub(crate) async fn telekio_ready(&self, interest: Interest) -> io::Result<(u8, Ready, bool)> {
        self.registration.telekio_ready(interest).await
    }

    pub(crate) fn try_telekio_ready(&self, interest: Interest) -> (u8, Ready, bool) {
        self.registration.try_telekio_ready(interest)
    }

    pub(crate) fn clear_telekio_ready(&self, tick: u8, ready: Ready) {
        self.registration.clear_telekio_ready(tick, ready);
    }
}
