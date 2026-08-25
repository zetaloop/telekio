#[derive(Debug)]
pub(super) struct SourceFd<'a>(pub &'a std::os::fd::RawFd);

impl std::os::fd::AsRawFd for SourceFd<'_> {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        *self.0
    }
}

impl mio::event::Source for SourceFd<'_> {
    fn register(
        &mut self,
        registry: &mio::Registry,
        token: mio::Token,
        interests: mio::Interest,
    ) -> std::io::Result<()> {
        mio::event::Source::register(&mut mio::unix::SourceFd(self.0), registry, token, interests)
    }

    fn reregister(
        &mut self,
        registry: &mio::Registry,
        token: mio::Token,
        interests: mio::Interest,
    ) -> std::io::Result<()> {
        mio::event::Source::reregister(&mut mio::unix::SourceFd(self.0), registry, token, interests)
    }

    fn deregister(&mut self, registry: &mio::Registry) -> std::io::Result<()> {
        mio::event::Source::deregister(&mut mio::unix::SourceFd(self.0), registry)
    }
}
