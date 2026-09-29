use std::{error::Error, path::Path};

use r#override::{item, root};

use super::{mount, patch};

pub(super) fn patch_time(generated: &Path) -> Result<(), Box<dyn Error>> {
    patch(&generated.join("src/time/sleep.rs"), |source| {
        source
            .select(root().import("Timer"))?
            .redirect("self::telekio::Timer")?;
        mount(source, None, "telekio", "guest/time/sleep.rs")
    })?;
    patch(&generated.join("src/time/clock.rs"), |source| {
        for (function, helper, context) in [
            ("pause", "telekio::pause", &[][..]),
            ("resume", "telekio::resume", &[][..]),
            ("advance", "telekio::advance", &["duration"][..]),
            ("now", "telekio::now", &[][..]),
        ] {
            source
                .select(item(function).call("with_clock").child(root().closure()))?
                .delegate(helper, context)?;
        }
        mount(source, None, "telekio", "guest/time/clock.rs")?;
        source
            .select(item("telekio"))?
            .add_attribute("#[cfg(feature = \"test-util\")]")
    })?;
    patch(&generated.join("src/runtime/mod.rs"), |source| {
        source
            .select(item("Timer"))?
            .add_attribute("#[expect(dead_code)]")?;
        source
            .select(root().implementation("Timer").has(item("new")))?
            .add_attribute("#[expect(dead_code)]")
    })
}
