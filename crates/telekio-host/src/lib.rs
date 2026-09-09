//! Tokio host runtime and plugin integration for Telekio.
#![allow(
    clippy::cognitive_complexity,
    clippy::io_other_error,
    clippy::large_enum_variant,
    clippy::manual_is_multiple_of,
    clippy::missing_safety_doc,
    clippy::module_inception,
    clippy::needless_doctest_main,
    clippy::needless_late_init,
    clippy::new_without_default,
    clippy::unnecessary_map_or
)]
#![warn(
    missing_debug_implementations,
    missing_docs,
    rust_2018_idioms,
    unreachable_pub
)]
#![deny(unused_must_use, unsafe_op_in_unsafe_fn)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
#![cfg_attr(docsrs, doc(auto_cfg(hide(loom))))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(docsrs, allow(unused_attributes))]
#![cfg_attr(loom, allow(dead_code, unreachable_pub))]
#![cfg_attr(windows, allow(rustdoc::broken_intra_doc_links))]

include!(env!("TELEKIO_TOKIO_SOURCE"));

cfg_rt! {
    cfg_sync! {
        mod bridge;

        pub use bridge::{Attach, Attachment, attach};
    }
}
