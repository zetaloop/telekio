use super::*;

impl<T: AsRawFd> AsyncFd<T> {
    #[doc(hidden)]
    pub fn poll_telekio_ready(
        &self,
        context: &mut Context<'_>,
        interest: Interest,
    ) -> Poll<io::Result<(u8, Ready, bool)>> {
        self.registration
            .poll_telekio_ready(context, interest.is_writable())
    }

    #[doc(hidden)]
    pub async fn telekio_ready(&self, interest: Interest) -> io::Result<(u8, Ready, bool)> {
        self.registration.telekio_ready(interest).await
    }

    #[doc(hidden)]
    pub fn try_telekio_ready(&self, interest: Interest) -> (u8, Ready, bool) {
        self.registration.try_telekio_ready(interest)
    }

    #[doc(hidden)]
    pub fn clear_telekio_ready(&self, tick: u8, ready: Ready) {
        self.registration.clear_telekio_ready(tick, ready);
    }
}
