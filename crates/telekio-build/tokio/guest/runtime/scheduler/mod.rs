use super::*;
use crate::runtime::{handle::telekio::Connection, task};
use std::sync::Arc;

pub(crate) trait HostSchedule: task::Schedule + Clone + Send + Sync + 'static {
    fn connection(&self) -> &Arc<Connection>;
    fn run(&self, call: impl FnOnce());
    fn enter<R>(&self, call: impl FnOnce() -> R) -> R;
}

macro_rules! host_schedule {
    ($variant:ident, $other:ident) => {
        impl Handle {
            pub(crate) fn owned_id(&self) -> std::num::NonZeroU64 {
                let _ = self.owned_id_inner();
                std::num::NonZeroU64::new(self.connection().handle.id())
                    .expect("invalid runtime ID")
            }

            pub(crate) fn install(
                &self,
                connection: std::sync::Arc<crate::runtime::handle::telekio::Connection>,
            ) {
                assert!(
                    self.telekio.set(connection).is_ok(),
                    "Tokio runtime was initialized twice"
                );
            }

            pub(crate) fn connection(
                &self,
            ) -> &std::sync::Arc<crate::runtime::handle::telekio::Connection> {
                self.telekio
                    .get()
                    .expect("Tokio runtime is not initialized")
            }
        }

        impl crate::runtime::scheduler::telekio::HostSchedule for std::sync::Arc<Handle> {
            fn connection(&self) -> &std::sync::Arc<crate::runtime::handle::telekio::Connection> {
                Handle::connection(self)
            }

            fn run(&self, call: impl FnOnce()) {
                let run = || {
                    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
                    crate::runtime::metrics::telekio::record_worker(self.connection());
                    call();
                };
                let current = crate::runtime::context::with_current(|handle| match handle {
                    crate::runtime::scheduler::Handle::$variant(handle) => {
                        std::sync::Arc::ptr_eq(handle, self)
                    }
                    #[cfg(feature = "rt-multi-thread")]
                    crate::runtime::scheduler::Handle::$other(_) => false,
                })
                .unwrap_or(false);
                if current {
                    crate::runtime::context::telekio::enter(run)
                } else {
                    let handle =
                        crate::runtime::scheduler::Handle::$variant(std::sync::Arc::clone(self));
                    let _guard = crate::runtime::context::try_set_current(&handle);
                    crate::runtime::context::telekio::enter(run)
                }
            }

            fn enter<R>(&self, call: impl FnOnce() -> R) -> R {
                let handle =
                    crate::runtime::scheduler::Handle::$variant(std::sync::Arc::clone(self));
                let _guard = crate::runtime::context::try_set_current(&handle);
                call()
            }
        }
    };
}

pub(crate) use host_schedule;

impl Handle {
    pub(crate) fn is_local(&self) -> bool {
        self.connection().handle.flavor() == ::telekio_abi::Flavor::Local
    }

    pub(crate) fn can_spawn_local_on_local_runtime(&self) -> bool {
        self.connection().handle.can_spawn_local()
    }

    pub(crate) fn connection(&self) -> &Arc<Connection> {
        match self {
            Handle::CurrentThread(handle) => handle.connection(),
            #[cfg(feature = "rt-multi-thread")]
            Handle::MultiThread(handle) => handle.connection(),
        }
    }

    #[track_caller]
    pub(crate) fn spawn_host_blocking<F, R>(&self, function: F) -> task::JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let id = task::Id::next();
        let spawned_at = task::SpawnLocation::capture();
        let location = task::SpawnLocation::take_telekio();
        match self {
            Handle::CurrentThread(handle) => {
                task::telekio::spawn_blocking(handle, function, id, spawned_at, location)
            }
            #[cfg(feature = "rt-multi-thread")]
            Handle::MultiThread(handle) => {
                task::telekio::spawn_blocking(handle, function, id, spawned_at, location)
            }
        }
    }
}
