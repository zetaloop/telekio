use super::{GlobalOrphanQueue, SignalHandle};

pub(super) fn reap_orphans(handle: &SignalHandle) {
    GlobalOrphanQueue::reap_orphans(handle);
    crate::process::unix::telekio::reap(handle);
}
