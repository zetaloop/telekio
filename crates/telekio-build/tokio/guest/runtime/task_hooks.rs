use std::sync::Arc;

use crate::runtime::{
    task::{self, SpawnLocation},
    TaskHooks, TaskMeta,
};

impl TaskHooks {
    pub(crate) fn spawn_host(&self, _: &TaskMeta<'_>) {}

    pub(crate) fn callback(self) -> ::telekio::TaskCallback {
        if !self.hooks_active() {
            return ::telekio::TaskCallback::none();
        }
        ::telekio::TaskCallback::from_arc(Arc::new(move |event, id, location| {
            let meta = TaskMeta {
                id: task::Id::from_telekio(id),
                spawned_at: SpawnLocation::from(location),
                _phantom: Default::default(),
            };
            match event {
                ::telekio::TaskEvent::Spawn => self.spawn(&meta),
                ::telekio::TaskEvent::PollStart => {
                    #[cfg(tokio_unstable)]
                    self.poll_start_callback(&meta);
                }
                ::telekio::TaskEvent::PollStop => {
                    #[cfg(tokio_unstable)]
                    self.poll_stop_callback(&meta);
                }
                ::telekio::TaskEvent::Terminate => {
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
