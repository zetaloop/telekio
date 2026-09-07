use super::*;
#[cfg(tokio_unstable)]
type Observer = std::sync::Arc<dyn Fn(usize) + Send + Sync>;

cfg_aio! {
    use std::{os::fd::BorrowedFd, sync::Arc};

    #[doc(hidden)]
    pub struct TelekioAio {
        handle: Handle,
        registration: Option<crate::runtime::io::telekio::Aio>,
    }

    impl TelekioAio {
        pub fn poll_ready(
            &self,
            context: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<(u8, crate::io::Ready, bool)>> {
            self.registration.as_ref().unwrap().poll_ready(context)
        }

        pub async fn ready(&self) -> std::io::Result<(u8, crate::io::Ready, bool)> {
            self.registration.as_ref().unwrap().ready().await
        }

        pub fn try_ready(&self) -> std::task::Poll<std::io::Result<(u8, crate::io::Ready, bool)>> {
            self.registration.as_ref().unwrap().try_ready()
        }

        pub fn clear_ready(&self, tick: u8, ready: crate::io::Ready) {
            self.registration.as_ref().unwrap().clear_ready(tick, ready);
        }
    }

    impl std::fmt::Debug for TelekioAio {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.debug_struct("TelekioAio").finish_non_exhaustive()
        }
    }

    impl Drop for TelekioAio {
        fn drop(&mut self) {
            if let Some(registration) = self.registration.take() {
                self.handle
                    .inner
                    .driver()
                    .io()
                    .telekio_deregister_aio(registration);
            }
        }
    }
}

cfg_io_driver! {
    #[cfg(target_os = "linux")]
    use std::{os::fd::RawFd, sync::Arc};

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
}

impl Handle {
    pub fn telekio_unhandled_panic(&self) {
        match &self.inner {
            scheduler::Handle::CurrentThread(handle) => {
                runtime::task::Schedule::unhandled_panic(handle)
            }
            #[cfg(feature = "rt-multi-thread")]
            scheduler::Handle::MultiThread(handle) => {
                runtime::task::Schedule::unhandled_panic(handle)
            }
        }
    }

    pub fn telekio_spawn<F>(
        &self,
        future: F,
        id: u64,
        location: Option<&'static std::panic::Location<'static>>,
    ) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        runtime::telekio::with_task(id, location, || self.spawn(future))
    }

    pub fn telekio_spawn_blocking<F, R>(
        &self,
        function: F,
        id: u64,
        location: Option<&'static std::panic::Location<'static>>,
    ) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        runtime::telekio::with_task(id, location, || self.spawn_blocking(function))
    }

    /// # Safety
    ///
    /// The future and output must remain on the LocalRuntime's owning thread.
    pub unsafe fn telekio_spawn_local<F>(
        &self,
        future: F,
        id: u64,
        location: Option<&'static std::panic::Location<'static>>,
    ) -> JoinHandle<F::Output>
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        let size = std::mem::size_of::<F>();
        runtime::telekio::with_task(id, location, || unsafe {
            self.spawn_local_named(future, SpawnMeta::new_unnamed(size))
        })
    }

    cfg_aio! {
        #[doc(hidden)]
        pub fn telekio_register_aio(
            &self,
            configure: Arc<dyn for<'a> Fn(BorrowedFd<'a>, usize) -> std::io::Result<()> + Send + Sync>,
            lio: bool,
        ) -> std::io::Result<TelekioAio> {
            let registration = self
                .inner
                .driver()
                .io()
                .telekio_register_aio(configure, lio)?;
            Ok(TelekioAio {
                handle: self.clone(),
                registration: Some(registration),
            })
        }
    }

    cfg_io_driver! {
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
    }

    #[cfg(feature = "rt-multi-thread")]
    #[doc(hidden)]
    pub fn telekio_record_poll(actual: u64, guest: u64) {
        crate::runtime::scheduler::multi_thread::telekio::record_poll(actual, guest);
    }

    #[cfg(tokio_unstable)]
    #[doc(hidden)]
    pub fn telekio_add_worker_observer(&self, observer: Observer) -> Option<u64> {
        match &self.inner {
            scheduler::Handle::CurrentThread(_) => {
                drop(observer);
                None
            }
            #[cfg(feature = "rt-multi-thread")]
            scheduler::Handle::MultiThread(handle) => {
                Some(handle.telekio_add_worker_observer(observer))
            }
        }
    }

    #[cfg(tokio_unstable)]
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
