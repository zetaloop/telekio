use super::*;
#[cfg(feature = "test-util")]
use crate::runtime::task::telekio::Host;
#[cfg(feature = "test-util")]
use std::sync::Arc;

cfg_taskdump! {
    pub(super) async fn dump<F>(
        handle: &Handle,
        original: F,
    ) -> crate::runtime::Dump
    where
        F: std::future::Future<Output = crate::runtime::Dump>,
    {
        match &handle.inner {
            scheduler::Handle::CurrentThread(inner)
                if inner.telekio.host().flavor() == ::telekio::Flavor::MultiThread =>
            {
                let tasks = inner
                    .telekio
                    .dump()
                    .await
                    .into_iter()
                    .map(|(id, trace)| crate::runtime::dump::Task::new(id, trace))
                    .collect();
                crate::runtime::Dump::new(tasks)
            }
            _ => original.await,
        }
    }
}

impl Handle {
    #[cfg(feature = "test-util")]
    pub(crate) fn host(&self) -> &Arc<Host> {
        self.inner.host()
    }

    /// Returns the flavor of the current runtime.
    pub fn runtime_flavor(&self) -> RuntimeFlavor {
        let _ = self.runtime_flavor_inner();
        match self.inner.host().flavor() {
            ::telekio::Flavor::CurrentThread | ::telekio::Flavor::Local => {
                RuntimeFlavor::CurrentThread
            }
            ::telekio::Flavor::MultiThread => RuntimeFlavor::MultiThread,
        }
    }
}
