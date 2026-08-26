use super::{StdChild, orphan::OrphanQueueImpl};

impl OrphanQueueImpl<StdChild> {
    pub(crate) fn reap_host_orphan(&self, orphan: StdChild) {
        let result = ::telekio::attached().reap_process(orphan.id());
        if result.into_io_result().is_err() {
            self.push_orphan(orphan);
        }
    }
}
