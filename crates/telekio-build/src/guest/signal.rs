#[cfg(test)]
pub(super) type RxFuture = crate::signal::RxFuture;

#[cfg(not(test))]
#[derive(Debug)]
pub(crate) struct RxFuture(::telekio::Signal);

#[cfg(not(test))]
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

#[cfg(all(unix, test))]
pub(crate) fn signal(
    kind: super::SignalKind,
    handle: &crate::runtime::signal::Handle,
) -> std::io::Result<crate::sync::watch::Receiver<()>> {
    super::signal_with_handle(kind, handle)
}

#[cfg(all(unix, not(test)))]
pub(crate) fn signal(
    kind: super::SignalKind,
    _: &crate::runtime::signal::Handle,
) -> std::io::Result<RxFuture> {
    crate::runtime::scheduler::Handle::current()
        .host()
        .signal(::telekio::SignalRequest::unix(kind.as_raw_value()))
        .map(RxFuture)
}

#[cfg(windows)]
macro_rules! console_signals {
    ($($name:ident => $kind:ident),+ $(,)?) => {
        $(
            #[cfg(test)]
            pub(crate) fn $name() -> std::io::Result<RxFuture> {
                super::imp::$name()
            }

            #[cfg(not(test))]
            pub(crate) fn $name() -> std::io::Result<RxFuture> {
                crate::runtime::scheduler::Handle::current()
                    .host()
                    .signal(::telekio::SignalRequest::console(::telekio::SignalKind::$kind))
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
