use std::{error::Error, process};

use sysinfo::{Pid, System};

pub fn offline() -> Result<bool, Box<dyn Error>> {
    let system = System::new_all();
    let mut process = system
        .process(Pid::from_u32(process::id()))
        .ok_or("Telekio build process is unavailable")?;
    while let Some(parent) = process.parent().and_then(|parent| system.process(parent)) {
        if parent.cmd().iter().any(|argument| {
            argument
                .to_str()
                .is_some_and(|argument| matches!(argument, "--offline" | "--frozen"))
        }) {
            return Ok(true);
        }
        process = parent;
    }
    Ok(false)
}
