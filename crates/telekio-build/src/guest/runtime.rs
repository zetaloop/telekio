use super::*;
use crate::runtime::{scheduler, task::telekio::Host};
use std::sync::Arc;

impl Runtime {
    pub(crate) fn install_host(&self, runtime: ::telekio::Runtime) {
        self.install(Host::new(runtime));
    }

    #[cfg(not(test))]
    pub(crate) fn install_attached_host(&self, handle: ::telekio::Handle) {
        self.install(Host::attached(handle));
    }

    #[cfg(not(test))]
    pub(crate) fn enter_attached<R>(
        &self,
        flavor: ::telekio::Flavor,
        call: impl FnOnce() -> R,
    ) -> R {
        if Handle::try_current().is_ok() {
            return call();
        }
        crate::runtime::context::enter_runtime(
            &self.handle.inner,
            flavor == ::telekio::Flavor::MultiThread,
            |_| crate::runtime::context::telekio::enter(|| crate::task::coop::budget(call)),
        )
    }

    fn install(&self, host: Arc<Host>) {
        match &self.handle.inner {
            scheduler::Handle::CurrentThread(handle) => handle.install(host.clone()),
            #[cfg(feature = "rt-multi-thread")]
            scheduler::Handle::MultiThread(handle) => handle.install(host.clone()),
        }
        self.blocking_pool.install(host);
    }
}
