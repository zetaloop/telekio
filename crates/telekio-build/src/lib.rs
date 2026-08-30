mod artifact;
mod compiler;
mod edit;
mod guest;
mod invocation;
mod linker;
mod source;

pub use guest::prepare_tests;
pub use source::{prepare_tokio, prepare_tokio_host};

#[doc(hidden)]
pub use artifact::{Artifact, init, remove};

#[macro_export]
macro_rules! init {
    (host) => {
        if $crate::init($crate::Artifact::Host) {
            return;
        }
    };
    (plugin) => {
        if $crate::init($crate::Artifact::Plugin) {
            return;
        }
    };
}

#[macro_export]
macro_rules! remove {
    () => {
        if $crate::remove() {
            return;
        }
    };
}
