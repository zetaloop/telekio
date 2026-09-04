use super::*;
use crate::runtime::{
    scheduler,
    task::telekio::{Host, HostTaskHooks},
};
use std::sync::Arc;

impl LocalRuntime {
    pub(crate) fn install_host(
        &self,
        runtime: ::telekio::Runtime,
        io_enabled: bool,
        task_hooks: Arc<HostTaskHooks>,
    ) {
        let host = Host::new(runtime, io_enabled, task_hooks);
        match &self.handle.inner {
            scheduler::Handle::CurrentThread(handle) => handle.install(host.clone()),
            #[cfg(feature = "rt-multi-thread")]
            scheduler::Handle::MultiThread(_) => unreachable!("LocalRuntime uses CurrentThread"),
        }
        self.blocking_pool.install(host);
    }
}
