use super::Handle;
use crate::runtime::scheduler::telekio::host_schedule;

host_schedule!(MultiThread, CurrentThread, true);

pub(super) fn host_worker<F>(worker: F)
where
    F: FnOnce() + Send + 'static,
{
    drop(worker);
}
