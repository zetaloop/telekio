use std::sync::Arc;

use crate::runtime::{
    task::{self, SpawnLocation},
    TaskHooks, TaskMeta,
};

impl TaskHooks {
    pub(crate) fn spawn_host(&self, _: &TaskMeta<'_>) {}

    pub(crate) fn callback(self) -> ::telekio_abi::TaskCallback {
        if !self.hooks_active() {
            return ::telekio_abi::TaskCallback::none();
        }
        ::telekio_abi::TaskCallback::from_arc(Arc::new(move |event, id, location| {
            let meta = TaskMeta {
                id: task::Id::from_telekio(id),
                spawned_at: SpawnLocation::from(location),
                _phantom: Default::default(),
            };
            match event {
                ::telekio_abi::TaskEvent::Spawn => self.spawn(&meta),
                ::telekio_abi::TaskEvent::PollStart => {
                    #[cfg(tokio_unstable)]
                    self.poll_start_callback(&meta);
                }
                ::telekio_abi::TaskEvent::PollStop => {
                    #[cfg(tokio_unstable)]
                    self.poll_stop_callback(&meta);
                }
                ::telekio_abi::TaskEvent::Terminate => {
                    if let Some(callback) = &self.task_terminate_callback {
                        callback(&meta);
                    }
                }
            }
        }))
    }

    fn hooks_active(&self) -> bool {
        self.task_spawn_callback.is_some() || self.task_terminate_callback.is_some() || {
            #[cfg(tokio_unstable)]
            {
                self.before_poll_callback.is_some() || self.after_poll_callback.is_some()
            }
            #[cfg(not(tokio_unstable))]
            {
                false
            }
        }
    }
}
