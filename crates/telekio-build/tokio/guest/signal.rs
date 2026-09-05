#[cfg(any(
    test,
    not(feature = "rt"),
    all(not(feature = "signal"), any(not(unix), not(feature = "process")))
))]
pub(super) type RxFuture = crate::signal::RxFuture;

#[cfg(all(
    not(test),
    feature = "rt",
    any(feature = "signal", all(unix, feature = "process"))
))]
#[derive(Debug)]
pub(crate) struct RxFuture(::telekio::Signal);

#[cfg(all(
    not(test),
    feature = "rt",
    any(feature = "signal", all(unix, feature = "process"))
))]
impl RxFuture {
    #[cfg(unix)]
    pub(crate) fn new(receiver: Self) -> Self {
        receiver
    }

    pub(crate) async fn recv(&mut self) {
        self.0.recv().await;
    }

    pub(crate) fn poll_recv(
        &mut self,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        self.0.poll_recv(context)
    }
}

#[cfg(all(
    unix,
    any(
        test,
        not(feature = "rt"),
        all(not(feature = "process"), not(feature = "signal"))
    )
))]
pub(crate) fn signal(
    kind: super::SignalKind,
    handle: &crate::runtime::signal::Handle,
) -> std::io::Result<crate::sync::watch::Receiver<()>> {
    super::signal_with_handle(kind, handle)
}

#[cfg(all(
    unix,
    not(test),
    feature = "rt",
    any(feature = "process", feature = "signal")
))]
#[track_caller]
pub(crate) fn signal(
    kind: super::SignalKind,
    _: &crate::runtime::signal::Handle,
) -> std::io::Result<RxFuture> {
    register(::telekio::SignalRequest::unix(kind.as_raw_value())).map(RxFuture)
}

#[cfg(all(
    feature = "rt",
    any(feature = "signal", all(unix, feature = "process"))
))]
#[cfg_attr(test, expect(dead_code))]
#[track_caller]
fn register(request: ::telekio::SignalRequest) -> std::io::Result<::telekio::Signal> {
    let handle = crate::runtime::scheduler::Handle::current();
    let connection = handle.connection();
    assert!(
        connection.io_enabled,
        "there is no signal driver running, must be called from the context of Tokio runtime"
    );
    connection.handle.signal(request)
}

#[cfg(windows)]
macro_rules! console_signals {
    ($($name:ident => $kind:ident),+ $(,)?) => {
        $(
            #[cfg(any(test, not(feature = "rt"), not(feature = "signal")))]
            pub(crate) fn $name() -> std::io::Result<RxFuture> {
                super::imp::$name()
            }

            #[cfg(all(not(test), feature = "rt", feature = "signal"))]
            #[track_caller]
            pub(crate) fn $name() -> std::io::Result<RxFuture> {
                register(::telekio::SignalRequest::console(::telekio::SignalKind::$kind))
                    .map(RxFuture)
            }
        )+
    };
}

#[cfg(windows)]
console_signals!(
    ctrl_c => CtrlC,
    ctrl_break => CtrlBreak,
    ctrl_close => CtrlClose,
    ctrl_logoff => CtrlLogoff,
    ctrl_shutdown => CtrlShutdown,
);
