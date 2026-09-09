use super::{orphan::OrphanQueueImpl, StdChild};

impl OrphanQueueImpl<StdChild> {
    pub(crate) fn reap_host_orphan(&self, orphan: StdChild) {
        if ::telekio_abi::attached()
            .reap_process(orphan.id())
            .into_io_result()
            .is_err()
        {
            self.push_orphan(orphan);
        }
    }
}
