use super::SpawnLocation;

mod runner;
pub(crate) use runner::spawn_blocking;

thread_local! {
    static SPAWN_LOCATION: std::cell::Cell<Option<::telekio::SourceLocation>> = const { std::cell::Cell::new(None) };
}

impl SpawnLocation {
    #[track_caller]
    pub(crate) fn capture() -> Self {
        SPAWN_LOCATION.set(Some(::telekio::SourceLocation::caller()));
        Self::capture_local()
    }

    pub(crate) fn take_telekio() -> ::telekio::SourceLocation {
        SPAWN_LOCATION
            .take()
            .expect("Tokio spawn location is missing")
    }
}
