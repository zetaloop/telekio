use super::*;

#[cfg(not(any(telekio_host, feature = "telekio-test")))]
fn attached() -> ::telekio::Handle {
    ::telekio::attached()
}

#[cfg(any(telekio_host, feature = "telekio-test"))]
fn host_owner() -> &'static ::telekio_host::Owner {
    use std::sync::OnceLock;

    static OWNER: OnceLock<::telekio_host::Owner> = OnceLock::new();
    OWNER.get_or_init(|| ::telekio_host::Runtime::new().unwrap().owner())
}

#[cfg(all(feature = "telekio-test", not(test)))]
#[unsafe(no_mangle)]
pub(super) extern "C-unwind" fn telekio_default_handle() -> ::telekio::RawHandle {
    host_owner().runtime().into_abi()
}

#[cfg(all(not(feature = "telekio-test"), any(telekio_host, not(test))))]
#[unsafe(no_mangle)]
pub(super) extern "C-unwind" fn telekio_default_handle() -> ::telekio::RawHandle {
    ::telekio::RawHandle::empty()
}

#[cfg(any(telekio_host, feature = "telekio-test"))]
fn attached() -> ::telekio::Handle {
    let handle = host_owner().runtime();
    if let Err(handle) = ::telekio::attach(handle) {
        drop(handle);
    }
    ::telekio::attached()
}

impl Builder {
    pub(super) fn build_hosted_current_thread(&mut self) -> io::Result<Runtime> {
        let host = self.build_host(false)?;
        let runtime = self.build_current_thread_runtime()?;
        runtime.install_host(host);
        Ok(runtime)
    }

    pub(super) fn build_hosted_local(&mut self) -> io::Result<LocalRuntime> {
        let host = self.build_host(true)?;
        let runtime = self.build_current_thread_local_runtime()?;
        runtime.install_host(host);
        Ok(runtime)
    }

    #[cfg(feature = "rt-multi-thread")]
    pub(super) fn build_hosted_multi_thread(&mut self) -> io::Result<Runtime> {
        let host = self.build_host(false)?;
        let runtime = self.build_threaded_runtime()?;
        runtime.install_host(host);
        Ok(runtime)
    }

    fn build_host(&self, local: bool) -> io::Result<::telekio::Runtime> {
        let flavor = if local {
            ::telekio::Flavor::Local
        } else {
            match self.kind {
                Kind::CurrentThread => ::telekio::Flavor::CurrentThread,
                #[cfg(feature = "rt-multi-thread")]
                Kind::MultiThread => ::telekio::Flavor::MultiThread,
            }
        };
        let keep_alive = self.keep_alive.unwrap_or_default();
        let result = attached().build(::telekio::RuntimeConfig {
            flavor,
            enable_io: self.enable_io.into(),
            enable_time: self.enable_time.into(),
            start_paused: self.start_paused.into(),
            worker_threads: self.worker_threads.unwrap_or_default(),
            max_blocking_threads: self.max_blocking_threads,
            thread_name: ::telekio::StringCallback::from_arc(self.thread_name.clone()),
            thread_stack_size: self.thread_stack_size.unwrap_or_default(),
            has_thread_stack_size: self.thread_stack_size.is_some().into(),
            after_start: self
                .after_start
                .clone()
                .map_or_else(::telekio::Callback::none, ::telekio::Callback::from_arc),
            before_stop: self
                .before_stop
                .clone()
                .map_or_else(::telekio::Callback::none, ::telekio::Callback::from_arc),
            before_park: self
                .before_park
                .clone()
                .map_or_else(::telekio::Callback::none, ::telekio::Callback::from_arc),
            after_unpark: self
                .after_unpark
                .clone()
                .map_or_else(::telekio::Callback::none, ::telekio::Callback::from_arc),
            keep_alive_secs: keep_alive.as_secs(),
            keep_alive_nanos: keep_alive.subsec_nanos(),
            has_keep_alive: self.keep_alive.is_some().into(),
            global_queue_interval: self.global_queue_interval.unwrap_or_default(),
            event_interval: self.event_interval,
            max_io_events_per_tick: self.nevents,
            name: unsafe { ::telekio::Bytes::borrow(self.name.as_deref()) },
        });
        // Tokio's LocalRuntime keeps this value on its originating thread.
        unsafe { result.into_runtime() }.map(|(runtime, _)| runtime)
    }
}
