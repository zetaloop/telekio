use std::{error::Error, path::Path};

use super::{mount, patch};
use crate::edit;

pub(super) fn patch_time(generated: &Path) -> Result<(), Box<dyn Error>> {
    patch(&generated.join("src/time/sleep.rs"), |source| {
        edit::retarget_use(source, "Timer", "self::telekio::Timer")?;
        mount(source, None, "telekio", "guest/time/sleep.rs")
    })?;
    patch(&generated.join("src/time/clock.rs"), |source| {
        edit::delegate_closure(
            source,
            edit::Scope::Function("pause"),
            edit::Call::Function("with_clock"),
            0,
            "telekio::pause",
            &[],
        )
        .map_err(|error| format!("pause clock integration: {error}"))?;
        edit::delegate_closure(
            source,
            edit::Scope::Function("resume"),
            edit::Call::Function("with_clock"),
            0,
            "telekio::resume",
            &[],
        )
        .map_err(|error| format!("resume clock integration: {error}"))?;
        edit::delegate_closure(
            source,
            edit::Scope::Function("advance"),
            edit::Call::Function("with_clock"),
            0,
            "telekio::advance",
            &["duration"],
        )
        .map_err(|error| format!("advance clock integration: {error}"))?;
        edit::delegate_closure(
            source,
            edit::Scope::Function("now"),
            edit::Call::Function("with_clock"),
            0,
            "telekio::now",
            &[],
        )?;
        mount(source, None, "telekio", "guest/time/clock.rs")?;
        edit::add_attr(
            source,
            edit::AttrTarget::Module("telekio"),
            "#[cfg(feature = \"test-util\")]",
        )
    })?;
    patch(&generated.join("src/runtime/mod.rs"), |source| {
        edit::add_attr(
            source,
            edit::AttrTarget::Enum("Timer"),
            "#[expect(dead_code)]",
        )?;
        edit::add_attr(
            source,
            edit::AttrTarget::Impl {
                owner: "Timer",
                method: "new",
            },
            "#[expect(dead_code)]",
        )
    })
}
