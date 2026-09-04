use super::super::SpawnLocation;
use super::Id;
use std::{cell::Cell, panic::Location};

thread_local! {
    static TASK_ID: Cell<Option<Id>> = const { Cell::new(None) };
    static TASK_LOCATION: Cell<Option<&'static Location<'static>>> = const { Cell::new(None) };
}

impl Id {
    pub(crate) fn from_telekio(value: u64) -> Self {
        Self(std::num::NonZeroU64::new(value).expect("invalid Tokio task ID"))
    }

    pub(crate) fn next() -> Self {
        TASK_ID.take().unwrap_or_else(Self::next_local)
    }

    #[doc(hidden)]
    pub fn telekio_value(self) -> u64 {
        self.0.get()
    }

    pub(crate) fn with_telekio<R>(
        value: u64,
        location: &'static Location<'static>,
        call: impl FnOnce() -> R,
    ) -> R {
        struct Reset {
            id: Option<Id>,
            location: Option<&'static Location<'static>>,
        }

        impl Drop for Reset {
            fn drop(&mut self) {
                TASK_ID.set(self.id);
                TASK_LOCATION.set(self.location);
            }
        }

        let reset = Reset {
            id: TASK_ID.replace(Some(Id::from_telekio(value))),
            location: TASK_LOCATION.replace(Some(location)),
        };
        assert!(
            reset.id.is_none(),
            "Tokio task ID override is already active"
        );
        assert!(
            reset.location.is_none(),
            "Tokio spawn location override is already active"
        );
        call()
    }
}

impl SpawnLocation {
    #[track_caller]
    pub(crate) fn capture() -> Self {
        TASK_LOCATION
            .take()
            .map(Self::from)
            .unwrap_or_else(Self::capture_local)
    }
}
