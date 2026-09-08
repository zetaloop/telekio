cfg_trace! {
    use super::{Instrumented, SpawnMeta};
    use tracing::Instrument;

    // The guest owns tracing for a forwarded operation.
    thread_local! {
        static FORWARDED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    pub(crate) fn task<F>(future: F, kind: &'static str, meta: SpawnMeta<'_>, id: u64) -> Instrumented<F> {
        if FORWARDED.take() {
            future.instrument(tracing::Span::none())
        } else {
            super::task(future, kind, meta, id)
        }
    }

    pub(crate) fn blocking_task<F, T>(future: T, meta: SpawnMeta<'_>, id: u64) -> Instrumented<T> {
        if FORWARDED.take() {
            future.instrument(tracing::Span::none())
        } else {
            super::blocking_task::<F, T>(future, meta, id)
        }
    }
}

cfg_not_trace! {
    pub(crate) use super::blocking_task;
}

pub(crate) fn forward<R>(call: impl FnOnce() -> R) -> R {
    #[cfg(all(tokio_unstable, feature = "tracing"))]
    let _reset = {
        struct Reset(bool);

        impl Drop for Reset {
            fn drop(&mut self) {
                FORWARDED.set(self.0);
            }
        }

        Reset(FORWARDED.replace(true))
    };
    call()
}
