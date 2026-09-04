use super::*;
use crate::runtime::task::telekio::host_schedule;

impl CurrentThread {
    #[track_caller]
    pub(crate) fn block_on<F: Future>(&self, handle: &scheduler::Handle, future: F) -> F::Output {
        crate::runtime::context::enter_runtime(handle, false, |_| {
            handle
                .as_current_thread()
                .telekio
                .host()
                .runtime_block_on(context::telekio::active(future))
        })
    }
}

host_schedule!(CurrentThread, MultiThread, false);
