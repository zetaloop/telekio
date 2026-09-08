use super::ScheduledIo;

#[derive(Debug, Default)]
pub(super) struct State {
    registration: Option<std::sync::Arc<::telekio_abi::IoRegistration>>,
    error: Option<std::io::Error>,
}

impl ScheduledIo {
    pub(crate) fn install(&self, registration: ::telekio_abi::IoRegistration) {
        let previous = self
            .telekio
            .lock()
            .unwrap()
            .registration
            .replace(std::sync::Arc::new(registration));
        assert!(previous.is_none(), "I/O resource was registered twice");
    }

    pub(crate) fn poll_telekio(
        &self,
        interest: ::telekio_abi::IoInterest,
        waker: &::telekio_abi::Waker,
    ) -> ::telekio_abi::IoPoll {
        self.registration().poll(interest, waker)
    }

    pub(crate) fn ready_telekio(
        &self,
        interest: ::telekio_abi::IoInterest,
    ) -> ::telekio_abi::IoOperation {
        self.registration().ready(interest)
    }

    #[cfg(windows)]
    pub(crate) fn try_operate_telekio(
        &self,
        request: ::telekio_abi::IoRequest,
    ) -> ::telekio_abi::IoPoll {
        self.registration().try_operate(request)
    }

    pub(crate) fn try_ready_telekio(
        &self,
        interest: ::telekio_abi::IoInterest,
    ) -> ::telekio_abi::IoPoll {
        self.registration().try_ready(interest)
    }

    pub(crate) fn clear_telekio(&self, tick: u8, ready: ::telekio_abi::IoReady) {
        if let Err(error) = self.registration().clear(tick, ready) {
            self.telekio.lock().unwrap().error.get_or_insert(error);
        }
    }

    pub(crate) fn clear_telekio_result(
        &self,
        tick: u8,
        ready: ::telekio_abi::IoReady,
    ) -> std::io::Result<()> {
        self.registration().clear(tick, ready)
    }

    pub(crate) fn take_telekio_error(&self) -> Option<std::io::Error> {
        self.telekio.lock().unwrap().error.take()
    }

    pub(crate) fn close_telekio(&self) {
        let registration = {
            let mut state = self.telekio.lock().unwrap();
            state.error.take();
            state.registration.take()
        };
        drop(registration);
    }

    fn registration(&self) -> std::sync::Arc<::telekio_abi::IoRegistration> {
        self.telekio
            .lock()
            .unwrap()
            .registration
            .as_ref()
            .expect("I/O resource is not registered")
            .clone()
    }
}
