use std::{
    env,
    error::Error,
    ffi::{OsStr, OsString},
    fs::{self, File},
    path::{Path, PathBuf},
    process::{self, Command},
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
        let manifest = project_manifest(&cargo, &arguments)?.ok_or("Cargo.toml was not found")?;
        if command == "init" {
            let mut workspace = arguments.clone();
            workspace.push(OsString::from("--workspace"));
            let (_, role) = project_info(&cargo, &workspace, &manifest)?;
            telekio_build::init_project(&manifest, role, offline(&arguments))?;
        } else {
            telekio_build::remove_project(&manifest)?;
        }
        return Ok(());
    }
    if command != "cargo" {
        return Err("usage: telekio <cargo|init|remove> [arguments]".into());
    }
    arguments.remove(0);
    let interpreted = expand_alias(&cargo, &arguments)?;
    let manifest = project_manifest(&cargo, &interpreted)?;
    let info = manifest
        .as_deref()
        .map(|manifest| project_info(&cargo, &interpreted, manifest))
        .transpose()?;
    if let Some((uses_tokio, _)) = info
        && !uses_tokio
    {
        return exit(Command::new(cargo).args(arguments).status()?);
    }

    let role = if operation(&interpreted) == Some("install") {
        Role::Host
    } else {
        info.map_or(Role::Host, |(_, role)| role)
    };
    let cache = cache_directory()?.join(match role {
        Role::Host => "host/tokio",
        Role::Guest => "guest/tokio",
    });
    let cache_parent = cache.parent().ok_or("patch cache has no parent")?;
    fs::create_dir_all(cache_parent)?;
    let lock = File::create(cache_parent.join("lock"))?;
    lock.lock()?;
    telekio_build::prepare_patch(&cache, role, offline(&interpreted))?;
    let config = write_config(&cache)?;
    drop(lock);

    let mut command = Command::new(cargo);
    if arguments
        .first()
        .and_then(|argument| argument.to_str())
        .is_some_and(|argument| argument.starts_with('+'))
    {
        command.arg(arguments.remove(0));
    }
    exit(
        command
            .arg("--config")
            .arg(config)
            .args(arguments)
            .status()?,
    )
}

fn offline(arguments: &[OsString]) -> bool {
    arguments
        .iter()
        .any(|argument| matches!(argument.to_str(), Some("--offline" | "--frozen")))
}

fn expand_alias(cargo: &OsStr, arguments: &[OsString]) -> Result<Vec<OsString>, Box<dyn Error>> {
    let mut expanded = arguments.to_vec();
    for _ in 0..16 {
        if operation(&expanded).is_some() {
            return Ok(expanded);
        }
        let output = Command::new(cargo)
            .args(global_options(&expanded))
            .args(["--color", "never", "--list"])
            .output()?;
        if !output.status.success() {
            return Ok(expanded);
        }
        let aliases = String::from_utf8(output.stdout)?;
        let Some((index, target)) = expanded.iter().enumerate().find_map(|(index, argument)| {
            let name = argument.to_str()?;
            aliases.lines().find_map(|line| {
                let (alias, target) = line.split_once(" alias: ")?;
                (alias.trim() == name).then_some((index, target))
            })
        }) else {
            return Ok(expanded);
        };
        if target.starts_with('!') {
            return Err("Cargo shell aliases cannot determine a Telekio artifact role; use the direct Cargo command".into());
        }
        expanded.splice(index..=index, target.split_whitespace().map(OsString::from));
    }
    Err("Cargo alias expansion is recursive".into())
}

fn project_info(
    cargo: &OsStr,
    arguments: &[OsString],
    manifest: &Path,
) -> Result<(bool, Role), Box<dyn Error>> {
    let output = Command::new(cargo)
        .args(metadata_options(arguments))
        .args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(manifest)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "Cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let packages = metadata["packages"]
        .as_array()
        .ok_or("Cargo metadata has no packages")?;
    let selected = selected_packages(&metadata, arguments, packages)?;
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .ok_or("Cargo metadata has no resolve nodes")?;
    let mut host = false;
    let mut guest = false;
    for dependencies in nodes
        .iter()
        .filter(|node| node["id"].as_str().is_some_and(|id| selected.contains(&id)))
        .filter_map(|node| node["deps"].as_array())
    {
        let mut package_host = false;
        let mut package_guest = false;
        for dependency in dependencies.iter().filter(|dependency| {
            dependency["dep_kinds"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind["kind"].is_null()))
        }) {
            let name = dependency["name"].as_str();
            let package = dependency["pkg"].as_str().and_then(|id| {
                packages
                    .iter()
                    .find(|package| package["id"].as_str() == Some(id))
                    .and_then(|package| package["name"].as_str())
            });
            match (name, package) {
                (Some("telekio_host"), _) | (_, Some("telekio-host")) => package_host = true,
                (_, Some("telekio")) => package_guest = true,
                _ => {}
            }
        }
        host |= package_host;
        guest |= package_guest && !package_host;
    }
    if host && guest {
        return Err("selected packages contain both Telekio host and guest artifacts".into());
    }
    let reachable = reachable_packages(&metadata, &selected)?;
    let uses_tokio = reachable.iter().any(|id| {
        packages.iter().any(|package| {
            package["id"].as_str() == Some(id) && package["name"].as_str() == Some("tokio")
        })
    });
    Ok((uses_tokio, if guest { Role::Guest } else { Role::Host }))
}

fn selected_packages<'a>(
    metadata: &'a serde_json::Value,
    arguments: &[OsString],
    packages: &'a [serde_json::Value],
) -> Result<Vec<&'a str>, Box<dyn Error>> {
    let workspace = has_option(arguments, "--workspace");
    let requested = option_values(arguments, &["-p", "--package"]);
    let members = metadata[if workspace {
        "workspace_members"
    } else {
        "workspace_default_members"
    }]
    .as_array()
    .ok_or("Cargo metadata has no workspace members")?;
    let mut selected: Vec<&str> = if workspace || requested.is_empty() {
        members
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect()
    } else {
        let names = requested
            .iter()
            .filter_map(|value| value.to_str())
            .map(|value| value.split_once('@').map_or(value, |(name, _)| name))
            .collect::<Vec<_>>();
        packages
            .iter()
            .filter(|package| {
                package["name"]
                    .as_str()
                    .is_some_and(|name| names.contains(&name))
            })
            .filter_map(|package| package["id"].as_str())
            .collect()
    };
    let excluded = option_values(arguments, &["--exclude"]);
    selected.retain(|id| {
        packages
            .iter()
            .find(|package| package["id"].as_str() == Some(id))
            .and_then(|package| package["name"].as_str())
            .is_none_or(|name| !excluded.iter().any(|excluded| *excluded == name))
    });
    if selected.is_empty() {
        return Err("Cargo package selection matched no packages".into());
    }
    Ok(selected)
}

fn reachable_packages<'a>(
    metadata: &'a serde_json::Value,
    selected: &[&'a str],
) -> Result<Vec<&'a str>, Box<dyn Error>> {
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .ok_or("Cargo metadata has no resolve nodes")?;
    let mut reachable = selected.to_vec();
    let mut index = 0;
    while index < reachable.len() {
        if let Some(node) = nodes
            .iter()
            .find(|node| node["id"].as_str() == Some(reachable[index]))
            && let Some(dependencies) = node["dependencies"].as_array()
        {
            for dependency in dependencies.iter().filter_map(serde_json::Value::as_str) {
                if !reachable.contains(&dependency) {
                    reachable.push(dependency);
                }
            }
        }
        index += 1;
    }
    Ok(reachable)
}

fn project_manifest(
    cargo: &OsStr,
    arguments: &[OsString],
) -> Result<Option<PathBuf>, Box<dyn Error>> {
    if operation(arguments) == Some("install") && !has_option(arguments, "--path") {
        return Ok(None);
    }
    let mut command = Command::new(cargo);
    command
        .args(global_options(arguments))
        .args(["locate-project", "--message-format", "plain"]);
    if let Some(path) = option(arguments, "--manifest-path") {
        command.arg("--manifest-path").arg(path);
    } else if let Some(path) = option(arguments, "--path") {
        command
            .arg("--manifest-path")
            .arg(Path::new(path).join("Cargo.toml"));
    }
    let output = command.output()?;
    if !output.status.success() {
        return Ok(None);
    }
    let path = String::from_utf8(output.stdout)?;
    Ok(Some(PathBuf::from(path.trim())))
}

fn metadata_options(arguments: &[OsString]) -> Vec<OsString> {
    let mut options = global_options(arguments)
        .into_iter()
        .map(OsStr::to_owned)
        .collect::<Vec<_>>();
    let mut index = 0;
    while index < arguments.len() && arguments[index] != "--" {
        let Some(argument) = arguments[index].to_str() else {
            index += 1;
            continue;
        };
        if matches!(argument, "--features" | "-F" | "--target") {
            if let Some(value) = arguments.get(index + 1) {
                options.push(OsString::from(if argument == "--target" {
                    "--filter-platform"
                } else {
                    argument
                }));
                options.push(value.clone());
            }
            index += 2;
            continue;
        }
        if argument.starts_with("--features=")
            || (argument.starts_with("-F") && argument.len() > "-F".len())
            || matches!(
                argument,
                "--all-features" | "--no-default-features" | "--locked" | "--frozen" | "--offline"
            )
        {
            options.push(arguments[index].clone());
        } else if let Some(target) = argument.strip_prefix("--target=") {
            options.push(OsString::from(format!("--filter-platform={target}")));
        }
        index += 1;
    }
    options
}

fn global_options(arguments: &[OsString]) -> Vec<&OsStr> {
    let mut options = Vec::new();
    let mut index = 0;
    while index < arguments.len() && arguments[index] != "--" {
        let Some(argument) = arguments[index].to_str() else {
            index += 1;
            continue;
        };
        if argument.starts_with('+') && index == 0 {
            options.push(arguments[index].as_os_str());
            index += 1;
            continue;
        }
        if argument == "--config" || argument == "-C" {
            if let Some(value) = arguments.get(index + 1) {
                options.extend([arguments[index].as_os_str(), value.as_os_str()]);
            }
            index += 2;
            continue;
        }
        if argument.starts_with("--config=") || argument.starts_with("-C=") {
            options.push(arguments[index].as_os_str());
            index += 1;
            continue;
        }
        index += 1;
    }
    options
}

fn operation(arguments: &[OsString]) -> Option<&str> {
    const COMMANDS: &[&str] = &[
        "bench",
        "build",
        "check",
        "clean",
        "clippy",
        "doc",
        "fetch",
        "fix",
        "info",
        "install",
        "metadata",
        "package",
        "publish",
        "run",
        "test",
        "tree",
        "uninstall",
        "update",
        "vendor",
    ];
    arguments
        .iter()
        .filter_map(|argument| argument.to_str())
        .find(|argument| COMMANDS.contains(argument))
}

fn option_values<'a>(arguments: &'a [OsString], names: &[&str]) -> Vec<&'a OsStr> {
    let mut values = Vec::new();
    for (index, argument) in arguments.iter().enumerate() {
        let Some(argument) = argument.to_str() else {
            continue;
        };
        for name in names {
            if argument == *name {
                if let Some(value) = arguments.get(index + 1) {
                    values.push(value.as_os_str());
                }
            } else if *name == "-p" && argument.starts_with("-p") && argument.len() > "-p".len() {
                values.push(OsStr::new(&argument["-p".len()..]));
            } else if let Some(value) = argument.strip_prefix(&format!("{name}=")) {
                values.push(OsStr::new(value));
            }
        }
    }
    values
}

fn has_option(arguments: &[OsString], name: &str) -> bool {
    arguments.iter().any(|argument| {
        argument
            .to_str()
            .is_some_and(|argument| argument == name || argument.starts_with(&format!("{name}=")))
    })
}

fn option<'a>(arguments: &'a [OsString], name: &str) -> Option<&'a OsStr> {
    for (index, argument) in arguments.iter().enumerate() {
        let Some(argument) = argument.to_str() else {
            continue;
        };
        if argument == name {
            return arguments.get(index + 1).map(OsString::as_os_str);
        }
        if let Some(value) = argument.strip_prefix(&format!("{name}=")) {
            return Some(OsStr::new(value));
        }
    }
    None
}

fn cache_directory() -> Result<PathBuf, Box<dyn Error>> {
    let directory = if cfg!(windows) {
        env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
    }
    .ok_or("user cache directory is unavailable")?
    .join("telekio");
    fs::create_dir_all(&directory)?;
    Ok(directory)
}

fn write_config(patch: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let path = patch
        .parent()
        .ok_or("patch directory has no parent")?
        .join("config.toml");
    let mut dependency = toml::Table::new();
    dependency.insert(
        "path".to_owned(),
        toml::Value::String(
            patch
                .to_str()
                .ok_or("patch path is not valid Unicode")?
                .to_owned(),
        ),
    );
    let config = toml::Value::Table(toml::Table::from_iter([(
        "patch".to_owned(),
        toml::Value::Table(toml::Table::from_iter([(
            "crates-io".to_owned(),
            toml::Value::Table(toml::Table::from_iter([(
                "tokio".to_owned(),
                toml::Value::Table(dependency),
            )])),
        )])),
    )]));
    fs::write(&path, toml::to_string(&config)?)?;
    Ok(path)
}

fn exit(status: process::ExitStatus) -> Result<(), Box<dyn Error>> {
    process::exit(status.code().unwrap_or(1));
}
