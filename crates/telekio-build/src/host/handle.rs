use super::*;
use std::sync::Arc;

type Observer = Arc<dyn Fn(usize) + Send + Sync>;

impl Handle {
    #[doc(hidden)]
    pub fn telekio_add_worker_observer(&self, observer: Observer) -> Option<u64> {
        match &self.inner {
            scheduler::Handle::CurrentThread(_) => None,
            #[cfg(feature = "rt-multi-thread")]
            scheduler::Handle::MultiThread(handle) => {
                Some(handle.telekio_add_worker_observer(observer))
            }
        }
    }

    #[doc(hidden)]
    pub fn telekio_remove_worker_observer(&self, id: Option<u64>) {
        #[cfg(feature = "rt-multi-thread")]
        if let (Some(id), scheduler::Handle::MultiThread(handle)) = (id, &self.inner) {
            handle.telekio_remove_worker_observer(id);
        }
        #[cfg(not(feature = "rt-multi-thread"))]
        let _ = id;
    }
}
