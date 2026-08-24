use super::*;
use crate::runtime::{scheduler, task::telekio::Host};

impl Runtime {
    pub(crate) fn install_host(&self, runtime: ::telekio::Runtime) {
        let host = Host::new(runtime);
        match &self.handle.inner {
            scheduler::Handle::CurrentThread(handle) => handle.install(host.clone()),
            #[cfg(feature = "rt-multi-thread")]
            scheduler::Handle::MultiThread(handle) => handle.install(host.clone()),
        }
        self.blocking_pool.install(host);
    }
}
