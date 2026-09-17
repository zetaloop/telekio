use std::{error::Error, process};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

pub fn offline() -> Result<bool, Box<dyn Error>> {
    let mut system = System::new();
    let refresh = ProcessRefreshKind::nothing().without_tasks();
    let pid = Pid::from_u32(process::id());
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, refresh);
    let parent = system
        .process(pid)
        .and_then(|process| process.parent())
        .ok_or("parent Cargo process is unavailable")?;
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[parent]),
        true,
        refresh.with_cmd(UpdateKind::OnlyIfNotSet),
    );
    let process = system
        .process(parent)
        .ok_or("parent Cargo process is unavailable")?;
    Ok(process
        .cmd()
        .iter()
        .take_while(|argument| *argument != "--")
        .any(|argument| {
            argument
                .to_str()
                .is_some_and(|argument| matches!(argument, "--offline" | "--frozen"))
        }))
}
