use super::*;
#[cfg(target_os = "linux")]
use std::os::fd::RawFd;
use std::sync::Arc;

type Observer = Arc<dyn Fn(usize) + Send + Sync>;

#[cfg(target_os = "linux")]
#[doc(hidden)]
pub struct TelekioIo {
    handle: Handle,
    registration: Option<crate::runtime::io::telekio::Registration>,
}

#[cfg(target_os = "linux")]
impl std::fmt::Debug for TelekioIo {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("TelekioIo").finish_non_exhaustive()
    }
}

#[cfg(target_os = "linux")]
impl Drop for TelekioIo {
    fn drop(&mut self) {
        if let Some(registration) = self.registration.take() {
            self.handle
                .inner
                .driver()
                .io()
                .telekio_deregister(registration);
        }
    }
}

impl Handle {
    #[cfg(target_os = "linux")]
    #[doc(hidden)]
    pub fn telekio_register_io(
        &self,
        fd: RawFd,
        callback: Arc<dyn Fn() + Send + Sync>,
    ) -> std::io::Result<TelekioIo> {
        let registration = self.inner.driver().io().telekio_register(fd, callback)?;
        Ok(TelekioIo {
            handle: self.clone(),
            registration: Some(registration),
        })
    }

    #[cfg(feature = "rt-multi-thread")]
    #[doc(hidden)]
    pub fn telekio_record_poll(actual: u64, guest: u64) {
        crate::runtime::scheduler::multi_thread::telekio::record_poll(actual, guest);
    }

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
