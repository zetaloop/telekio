use std::{collections::HashSet, error::Error, process};

use sysinfo::{CpuRefreshKind, MemoryRefreshKind, Pid, ProcessRefreshKind, ProcessesToUpdate, System};

pub fn offline() -> Result<bool, Box<dyn Error>> {
    eprintln!("offline: initialize system");
    let mut system = System::new();
    eprintln!("offline: refresh memory");
    system.refresh_memory_specifics(MemoryRefreshKind::everything());
    eprintln!("offline: refresh CPU");
    system.refresh_cpu_specifics(CpuRefreshKind::everything());
    eprintln!("offline: refresh processes");
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::everything());
    eprintln!("offline: inspect ancestors");
    let mut process = system
        .process(Pid::from_u32(process::id()))
        .ok_or("Telekio build process is unavailable")?;
    let mut ancestors = HashSet::new();
    while let Some(parent) = process.parent().and_then(|parent| system.process(parent)) {
        eprintln!("offline: {} -> {} ({:?})", process.pid(), parent.pid(), parent.name());
        assert!(ancestors.insert(parent.pid()), "process ancestry repeats");
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
