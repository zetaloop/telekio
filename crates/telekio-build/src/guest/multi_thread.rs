use super::*;
use crate::runtime::{context, task::telekio::host_schedule};

impl MultiThread {
    #[track_caller]
    pub(crate) fn block_on<F>(&self, handle: &scheduler::Handle, future: F) -> F::Output
    where
        F: Future,
    {
        crate::runtime::context::enter_runtime(handle, true, |_| match handle {
            scheduler::Handle::MultiThread(handle) => handle
                .telekio
                .host()
                .runtime_block_on(context::telekio::active(future)),
            _ => unreachable!("expected MultiThread scheduler"),
        })
    }
}

host_schedule!(MultiThread, CurrentThread, true);

pub(super) fn host_worker<F>(worker: F)
where
    F: FnOnce() + Send + 'static,
{
    drop(worker);
}
