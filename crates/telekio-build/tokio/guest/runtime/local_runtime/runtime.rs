use super::*;
use crate::runtime::{handle::telekio::Connection, scheduler};

impl LocalRuntime {
    pub(crate) fn install_host(&self, runtime: ::telekio_abi::Runtime, io_enabled: bool) {
        let connection = Connection::new(runtime.handle(), io_enabled);
        #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
        connection.workers.store(0);
        match &self.handle.inner {
            scheduler::Handle::CurrentThread(handle) => handle.install(connection),
            #[cfg(feature = "rt-multi-thread")]
            scheduler::Handle::MultiThread(_) => unreachable!("LocalRuntime uses CurrentThread"),
        }
        self.blocking_pool.install(runtime);
    }
}
