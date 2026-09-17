use std::{error::Error, process};

use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System, UpdateKind};

pub fn offline() -> Result<bool, Box<dyn Error>> {
    eprintln!("offline: snapshot start");
    let system = System::new_with_specifics(
        RefreshKind::nothing().with_processes(
            ProcessRefreshKind::nothing()
                .with_cmd(UpdateKind::OnlyIfNotSet)
                .without_tasks(),
        ),
    );
    eprintln!("offline: snapshot ready");
    let mut seen = std::collections::HashSet::new();
    let mut process = system
        .process(Pid::from_u32(process::id()))
        .ok_or("Telekio build process is unavailable")?;
    while let Some(parent) = process.parent().and_then(|parent| system.process(parent)) {
        eprintln!("offline: {} {:?} -> {} {:?}", process.pid(), process.name(), parent.pid(), parent.name());
        assert!(seen.insert(parent.pid()), "process ancestry repeats");
        if parent.cmd().iter().any(|argument| {
            argument
                .to_str()
                .is_some_and(|argument| matches!(argument, "--offline" | "--frozen"))
        }) {
            return Ok(true);
        }
        process = parent;
    }
    eprintln!("offline: complete");
    Ok(false)
}
