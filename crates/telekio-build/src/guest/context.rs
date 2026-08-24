use super::*;
use std::future::Future;

impl BlockingRegionGuard {
    pub(crate) fn block_on_host<F>(&mut self, future: F) -> Result<F::Output, ()>
    where
        F: Future,
    {
        Ok(crate::runtime::scheduler::Handle::current()
            .host()
            .handle_block_on(future))
    }
}
