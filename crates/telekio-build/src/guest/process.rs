use super::{StdChild, orphan::OrphanQueueImpl};

impl OrphanQueueImpl<StdChild> {
    pub(crate) fn reap_host_orphan(&self, orphan: StdChild) {
        #[cfg(any(telekio_host, feature = "telekio-test"))]
        ::telekio_host::reap_process(orphan.id());
        #[cfg(not(any(telekio_host, feature = "telekio-test")))]
        if ::telekio::attached()
            .reap_process(orphan.id())
            .into_io_result()
            .is_err()
        {
            self.push_orphan(orphan);
        }
    }
}
