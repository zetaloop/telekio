use super::Id;
use std::cell::Cell;

thread_local! {
    static TASK_ID: Cell<Option<Id>> = const { Cell::new(None) };
}

impl Id {
    pub(crate) fn next() -> Self {
        TASK_ID.take().unwrap_or_else(Self::next_local)
    }

    pub(crate) fn telekio_value(self) -> u64 {
        self.0.get()
    }

    pub(crate) fn with_telekio<R>(value: u64, call: impl FnOnce() -> R) -> R {
        struct Reset(Option<Id>);

    impl Drop for Reset {
        fn drop(&mut self) {
            TASK_ID.set(self.0);
        }
    }

        let id = Id(std::num::NonZeroU64::new(value).expect("invalid Tokio task ID"));
        let reset = Reset(TASK_ID.replace(Some(id)));
        assert!(reset.0.is_none(), "Tokio task ID override is already active");
        call()
    }
}
