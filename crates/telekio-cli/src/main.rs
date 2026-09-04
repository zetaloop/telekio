mod cargo;
mod project;

use std::{
    env,
    error::Error,
    ffi::{OsStr, OsString},
    fs::{self, File},
    path::{Path, PathBuf},
    process,
};

use telekio_build::Role;

fn main() {
    if let Err(error) = run() {
        eprintln!("telekio: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args_os().skip(1).collect::<Vec<_>>();
    let command = arguments
        .first()
        .and_then(|argument| argument.to_str())
        .ok_or("usage: telekio <cargo|init|remove> [arguments]")?
        .to_owned();
    if matches!(command.as_str(), "--help" | "-h") {
        println!("Usage: telekio <cargo|init|remove> [arguments]");
        return Ok(());
    }
    if matches!(command.as_str(), "--version" | "-V") {
        println!("telekio {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let cargo = OsString::from("cargo");
    if matches!(command.as_str(), "init" | "remove") {
        arguments.remove(0);
        let manifest = cargo::manifest(&cargo, &arguments)?.ok_or("Cargo.toml was not found")?;
        if command == "init" {
            let mut workspace = arguments.clone();
            workspace.push(OsString::from("--workspace"));
            let info = project::info(&cargo, &workspace, &manifest)?;
            let role = info.role().ok_or("a persistent Tokio patch cannot represent both Telekio host and guest packages; use `telekio cargo` for this workspace")?;
            project::init(&manifest, role, cargo::offline(&arguments))?;
        } else {
            project::remove(&manifest)?;
        }
        return Ok(());
    }
    if command != "cargo" {
        return Err("usage: telekio <cargo|init|remove> [arguments]".into());
    }
    arguments.remove(0);
    let interpreted = cargo::expand_alias(&cargo, &arguments)?;
    let operation = cargo::operation(&interpreted);
    if operation.is_some_and(|operation| cargo::kind(operation) == cargo::Kind::Plain) {
        return exit(cargo::status(&cargo, &arguments, None)?);
    }
    let manifest = cargo::manifest(&cargo, &interpreted)?;
    let Some(manifest) = manifest else {
        let config = prepare_config(Role::Host, false, cargo::offline(&interpreted))?;
        return exit(cargo::status(&cargo, &interpreted, Some(&config))?);
    };
    let info = project::info(&cargo, &interpreted, &manifest)?;
    if let Some(role) = info.role() {
        let config = prepare_config(
            role,
            role == Role::Guest && info.workspace_mixed,
            cargo::offline(&interpreted),
        )?;
        let selected = project::verify_patch(
            &cargo,
            &interpreted,
            &manifest,
            &config,
            &info.package_names(),
        )?;
        return if selected {
            exit(cargo::status(&cargo, &interpreted, Some(&config))?)
        } else {
            exit(cargo::status(&cargo, &arguments, None)?)
        };
    }

    let operation = operation.ok_or(
        "this Cargo extension cannot determine mixed Telekio package selection; select one role",
    )?;
    match cargo::kind(operation) {
        cargo::Kind::Packages if info.workspace_selection => {
            let (host, guest) = info.groups();
            run_groups(&cargo, &interpreted, &manifest, host, guest, true)
        }
        cargo::Kind::Workspace => {
            let (host, guest) = info.groups();
            run_groups(&cargo, &interpreted, &manifest, host, guest, false)
        }
        cargo::Kind::Packages if operation == "tree" => {
            let (host, guest) = info.groups();
            run_groups(&cargo, &interpreted, &manifest, host, guest, false)
        }
        cargo::Kind::Single => Err(format!(
            "`cargo {operation}` requires packages from one Telekio role; select one package"
        )
        .into()),
        cargo::Kind::Output | cargo::Kind::Packages => Err(format!(
            "`cargo {operation}` cannot combine Telekio host and guest dependency graphs; select one package role"
        )
        .into()),
        cargo::Kind::Plain => unreachable!(),
    }
}

fn run_groups(
    cargo: &OsStr,
    arguments: &[OsString],
    manifest: &Path,
    host: &[String],
    guest: &[String],
    select_packages: bool,
) -> Result<(), Box<dyn Error>> {
    let mut failed = None;
    for (role, packages) in [(Role::Host, host), (Role::Guest, guest)] {
        if packages.is_empty() {
            continue;
        }
        let arguments = if select_packages {
            cargo::package_arguments(arguments, packages)
        } else {
            arguments.to_vec()
        };
        let config = prepare_config(role, true, cargo::offline(&arguments))?;
        let selected = project::verify_patch(cargo, &arguments, manifest, &config, packages)?;
        let status = cargo::status(cargo, &arguments, selected.then_some(config.as_path()))?;
        if !status.success() {
            if !cargo::has_option(&arguments, "--keep-going")
                && !cargo::has_option(&arguments, "--no-fail-fast")
            {
                return exit(status);
            }
            failed.get_or_insert(status);
        }
    }
    if let Some(status) = failed {
        exit(status)
    } else {
        Ok(())
    }
}

fn prepare_config(role: Role, mixed: bool, offline: bool) -> Result<PathBuf, Box<dyn Error>> {
    let cache = cache_directory()?.join(match (role, mixed) {
        (Role::Host, _) => "host/tokio",
        (Role::Guest, false) => "guest/tokio",
        (Role::Guest, true) => "mixed/guest/tokio",
    });
    let cache_parent = cache.parent().ok_or("patch cache has no parent")?;
    fs::create_dir_all(cache_parent)?;
    let lock = File::create(cache_parent.join("lock"))?;
    lock.lock()?;
    if role == Role::Guest && mixed {
        telekio_build::prepare_mixed_guest_patch(&cache, offline)?;
    } else {
        telekio_build::prepare_patch(&cache, role, offline)?;
    }
    let config = write_config(&cache)?;
    drop(lock);
    Ok(config)
}

fn cache_directory() -> Result<PathBuf, Box<dyn Error>> {
    let directory = env::temp_dir().join("telekio");
    fs::create_dir_all(&directory)?;
    Ok(directory)
}

fn write_config(patch: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let path = patch
        .parent()
        .ok_or("patch directory has no parent")?
        .join("config.toml");
    let patch = serde_json::to_string(patch.to_str().ok_or("patch path is not valid Unicode")?)?;
    fs::write(&path, format!("[patch.crates-io.tokio]\npath = {patch}\n"))?;
    Ok(path)
}

fn exit(status: process::ExitStatus) -> Result<(), Box<dyn Error>> {
    process::exit(status.code().unwrap_or(1));
}
