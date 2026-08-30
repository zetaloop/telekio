use super::*;
use crate::io::Ready;

impl Registration {
    pub(crate) fn poll_telekio_ready(
        &self,
        context: &mut Context<'_>,
        write: bool,
    ) -> Poll<io::Result<(u8, Ready, bool)>> {
        let event = if write {
            ready!(self.poll_write_ready(context))?
        } else {
            ready!(self.poll_read_ready(context))?
        };
        Poll::Ready(Ok(telekio_event(event)))
    }

    pub(crate) async fn telekio_ready(&self, interest: Interest) -> io::Result<(u8, Ready, bool)> {
        self.readiness(interest).await.map(telekio_event)
    }

    pub(crate) fn try_telekio_ready(&self, interest: Interest) -> (u8, Ready, bool) {
        telekio_event(self.shared.ready_event(interest))
    }

    pub(crate) fn clear_telekio_ready(&self, tick: u8, ready: Ready) {
        self.clear_readiness(ReadyEvent {
            tick,
            ready,
            is_shutdown: false,
        });
    }
}

fn telekio_event(event: ReadyEvent) -> (u8, Ready, bool) {
    (event.tick, event.ready, event.is_shutdown)
}
