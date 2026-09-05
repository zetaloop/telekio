use super::*;
use std::future::Future;

impl BlockingRegionGuard {
    pub(crate) fn block_on_host<F>(&mut self, future: F) -> Result<F::Output, ()>
    where
        F: Future,
    {
        let handle = crate::runtime::scheduler::Handle::current();
        let connection = handle.connection();
        let future = crate::runtime::context::telekio::active(future);
        #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
        let future = crate::runtime::metrics::telekio::root(connection, future);
        Ok(connection.handle.block_on(future))
    }
}
