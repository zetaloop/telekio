use super::{Clock, Instant};
use crate::time::Duration;

#[cfg(feature = "rt")]
use crate::runtime::Handle;
pub(super) fn pause<F>(guest: F) -> impl FnOnce(Option<&Clock>) -> Result<(), &'static str>
where
    F: FnOnce(Option<&Clock>) -> Result<(), &'static str>,
{
    move |clock| {
        guest(clock)?;
        #[cfg(feature = "rt")]
        Handle::current()
            .connection()
            .handle
            .pause()
            .into_io_result()
            .unwrap();
        Ok(())
    }
}

pub(super) fn resume<F>(guest: F) -> impl FnOnce(Option<&Clock>) -> Result<(), &'static str>
where
    F: FnOnce(Option<&Clock>) -> Result<(), &'static str>,
{
    move |clock| {
        guest(clock)?;
        #[cfg(feature = "rt")]
        Handle::current()
            .connection()
            .handle
            .resume()
            .into_io_result()
            .unwrap();
        Ok(())
    }
}

pub(super) fn advance<F>(
    duration: Duration,
    guest: F,
) -> impl FnOnce(Option<&Clock>) -> Result<(), &'static str>
where
    F: FnOnce(Option<&Clock>) -> Result<(), &'static str>,
{
    #[cfg(not(feature = "rt"))]
    let _ = duration;
    move |clock| {
        guest(clock)?;
        #[cfg(feature = "rt")]
        Handle::current()
            .connection()
            .handle
            .advance(duration)
            .into_io_result()
            .unwrap();
        Ok(())
    }
}

pub(super) fn now<F>(guest: F) -> impl FnOnce(Option<&Clock>) -> Result<Instant, &'static str>
where
    F: FnOnce(Option<&Clock>) -> Result<Instant, &'static str>,
{
    move |clock| {
        #[cfg(feature = "rt")]
        if clock.is_some() {
            return Ok(Instant::from_std(
                Handle::current().connection().handle.now(),
            ));
        }
        guest(clock)
    }
}
