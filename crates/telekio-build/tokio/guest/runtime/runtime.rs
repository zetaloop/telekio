use super::*;
use crate::runtime::{handle::telekio::Connection, scheduler};
use std::sync::Arc;

#[track_caller]
pub(crate) fn block_on<F: Future>(
    pool: &BlockingPool,
    _: &impl Sized,
    handle: &scheduler::Handle,
    future: F,
) -> F::Output {
    let allow_block_in_place = match handle {
        scheduler::Handle::CurrentThread(_) => false,
        #[cfg(feature = "rt-multi-thread")]
        scheduler::Handle::MultiThread(_) => true,
    };
    crate::runtime::context::enter_runtime(handle, allow_block_in_place, |_| {
        let future = crate::runtime::context::telekio::active(future);
        #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
        let future = crate::runtime::metrics::telekio::root(handle.connection(), future);
        pool.runtime().block_on(future)
    })
}

impl Runtime {
    pub(crate) fn install_host(&self, runtime: ::telekio_abi::Runtime, io_enabled: bool) {
        self.install(Connection::new(runtime.handle(), io_enabled));
        self.blocking_pool.install(runtime);
    }

    #[cfg(not(test))]
    pub(crate) fn install_attached_host(&self, handle: ::telekio_abi::Handle) {
        self.install(Connection::new(handle, true));
    }

    #[cfg(not(test))]
    pub(crate) fn enter_attached<R>(
        &self,
        flavor: ::telekio_abi::Flavor,
        call: impl FnOnce() -> R,
    ) -> R {
        if Handle::try_current().is_ok() {
            return call();
        }
        crate::runtime::context::enter_runtime(
            &self.handle.inner,
            flavor == ::telekio_abi::Flavor::MultiThread,
            |_| crate::runtime::context::telekio::enter(call),
        )
    }

    fn install(&self, connection: Arc<Connection>) {
        match &self.handle.inner {
            scheduler::Handle::CurrentThread(handle) => handle.install(connection),
            #[cfg(feature = "rt-multi-thread")]
            scheduler::Handle::MultiThread(handle) => handle.install(connection),
        }
    }
}
