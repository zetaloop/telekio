use super::ScheduledIo;

impl ScheduledIo {
    pub(crate) fn install(&self, registration: ::telekio::IoRegistration) {
        let previous = self.telekio.lock().unwrap().replace(registration);
        assert!(previous.is_none(), "I/O resource was registered twice");
    }

    pub(crate) fn poll_telekio(
        &self,
        interest: ::telekio::IoInterest,
        waker: &::telekio::Waker,
    ) -> ::telekio::IoPoll {
        self.telekio
            .lock()
            .unwrap()
            .as_ref()
            .expect("I/O resource is not registered")
            .poll(interest, waker)
    }

    pub(crate) fn ready_telekio(&self, interest: ::telekio::IoInterest) -> ::telekio::IoOperation {
        self.telekio
            .lock()
            .unwrap()
            .as_ref()
            .expect("I/O resource is not registered")
            .ready(interest)
    }

    pub(crate) fn try_operate_telekio(&self, request: ::telekio::IoRequest) -> ::telekio::IoPoll {
        self.telekio
            .lock()
            .unwrap()
            .as_ref()
            .expect("I/O resource is not registered")
            .try_operate(request)
    }

    pub(crate) fn try_ready_telekio(&self, interest: ::telekio::IoInterest) -> ::telekio::IoPoll {
        self.telekio
            .lock()
            .unwrap()
            .as_ref()
            .expect("I/O resource is not registered")
            .try_ready(interest)
    }

    pub(crate) fn clear_telekio(&self, ready: ::telekio::IoReady) {
        self.telekio
            .lock()
            .unwrap()
            .as_ref()
            .expect("I/O resource is not registered")
            .clear(ready);
    }

    pub(crate) fn close_telekio(&self) {
        self.telekio.lock().unwrap().take();
    }
}
