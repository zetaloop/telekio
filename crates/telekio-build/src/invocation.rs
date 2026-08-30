use std::{
    env,
    error::Error,
    ffi::OsString,
    path::{Path, PathBuf},
    process::{self, Command},
};

use sysinfo::{Pid, System};

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Operation {
    Build,
    Check,
    Clippy,
    Test,
    Bench,
    Doc,
    Run,
    Install,
    Package,
    Publish,
}

impl Operation {
    pub fn name(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Check => "check",
            Self::Clippy => "clippy",
            Self::Test => "test",
            Self::Bench => "bench",
            Self::Doc => "doc",
            Self::Run => "run",
            Self::Install => "install",
            Self::Package => "package",
            Self::Publish => "publish",
        }
    }

    pub fn building(self) -> bool {
        matches!(
            self,
            Self::Build | Self::Run | Self::Install | Self::Package | Self::Publish
        )
    }

    fn uses_build(self) -> bool {
        matches!(
            self,
            Self::Run | Self::Install | Self::Package | Self::Publish
        )
    }
}

pub struct Invocation {
    arguments: Vec<OsString>,
    command: usize,
    operation: Operation,
    alias_options: Vec<OsString>,
    aliased: bool,
    cwd: PathBuf,
}

pub struct Inner {
    pub global: Vec<OsString>,
    pub command: OsString,
    pub options: Vec<OsString>,
}

impl Invocation {
    pub fn detect() -> Result<Self, Box<dyn Error>> {
        let system = System::new_all();
        let process = system
            .process(Pid::from_u32(process::id()))
            .and_then(|process| process.parent())
            .and_then(|parent| system.process(parent))
            .ok_or("parent Cargo process is unavailable")?;
        let mut invocation = Self::parse(
            process.cmd().to_vec(),
            process
                .cwd()
                .ok_or("parent Cargo working directory is unavailable")?,
        )?;
        if invocation.operation == Operation::Check
            && let Some(clippy) = process.parent().and_then(|parent| system.process(parent))
            && clippy.name().to_string_lossy().starts_with("cargo-clippy")
        {
            invocation = Self::parse(
                clippy.cmd().to_vec(),
                clippy
                    .cwd()
                    .ok_or("cargo-clippy working directory is unavailable")?,
            )?;
        }
        Ok(invocation)
    }

    fn parse(arguments: Vec<OsString>, cwd: &Path) -> Result<Self, Box<dyn Error>> {
        let command = command_index(&arguments)?;
        let token = arguments[command]
            .to_str()
            .ok_or("Cargo subcommand is not Unicode")?;
        let mut global = arguments[1..command].to_vec();
        global.extend(selected_options(
            arguments[command + 1..]
                .iter()
                .take_while(|argument| *argument != "--"),
            "--config",
        ));
        let aliased = operation(token).is_none();
        let (resolved, alias_options) = resolve(token, &global, cwd, &mut Vec::new())?;
        Ok(Self {
            arguments,
            command,
            operation: resolved,
            alias_options,
            aliased,
            cwd: cwd.to_owned(),
        })
    }

    pub fn operation(&self) -> Operation {
        self.operation
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn configs(&self) -> Vec<OsString> {
        selected_options(self.cargo_options().into_iter(), "--config")
            .into_iter()
            .map(|option| absolute_config(option, &self.cwd))
            .collect()
    }

    pub fn resolution(&self) -> Vec<OsString> {
        self.cargo_options()
            .into_iter()
            .filter(|argument| {
                matches!(
                    argument.to_str(),
                    Some("--locked" | "--frozen" | "--offline")
                )
            })
            .cloned()
            .collect()
    }

    pub fn offline(&self) -> bool {
        self.cargo_options()
            .into_iter()
            .any(|argument| matches!(argument.to_str(), Some("--offline" | "--frozen")))
    }

    pub fn verbose(&self) -> bool {
        self.cargo_options().into_iter().any(|argument| {
            argument
                .to_str()
                .is_some_and(|argument| argument == "--verbose" || argument.starts_with("-v"))
        })
    }

    pub fn inner(&self) -> Result<Inner, Box<dyn Error>> {
        let uses_build = self.operation.uses_build();
        let command = if uses_build {
            OsString::from("build")
        } else if self.aliased {
            OsString::from(self.operation.name())
        } else {
            self.arguments[self.command].clone()
        };
        let mut source = self.alias_options.clone();
        source.extend(self.arguments.iter().skip(self.command + 1).cloned());
        let mut options = Vec::new();
        let mut index = 0;
        while index < source.len() {
            let argument = source[index].to_string_lossy();
            if argument == "--config" {
                index += 2;
                continue;
            }
            if argument.starts_with("--config=") {
                index += 1;
                continue;
            }
            if argument == "--"
                && matches!(
                    self.operation,
                    Operation::Run | Operation::Install | Operation::Package | Operation::Publish
                )
            {
                break;
            }
            if let Some(takes_value) = removed_option(self.operation, &argument) {
                index += usize::from(takes_value && !argument.contains('=')) + 1;
                continue;
            }
            if self.operation == Operation::Install
                && !argument.starts_with('-')
                && !previous_may_take_value(self.operation, &source, index)
            {
                index += 1;
                continue;
            }
            options.push(source[index].clone());
            index += 1;
        }
        let cargo_options = self.cargo_options();
        let mut generated = vec![
            OsString::from("--package"),
            env::var_os("CARGO_PKG_NAME").ok_or("CARGO_PKG_NAME is unavailable")?,
        ];
        if env::var("PROFILE").as_deref() == Ok("release")
            && self.operation != Operation::Bench
            && !cargo_options.iter().any(|argument| {
                matches!(argument.to_str(), Some("-r" | "--release" | "--profile"))
                    || argument.to_string_lossy().starts_with("--profile=")
            })
        {
            generated.push("--release".into());
        }
        if !cargo_options.iter().any(|argument| {
            *argument == "--target" || argument.to_string_lossy().starts_with("--target=")
        }) {
            generated.extend([
                OsString::from("--target"),
                env::var_os("TARGET").ok_or("TARGET is unavailable")?,
            ]);
        }
        let separator = options
            .iter()
            .position(|argument| argument == "--")
            .unwrap_or(options.len());
        options.splice(separator..separator, generated);
        let mut global = Vec::new();
        let mut index = 1;
        while index < self.command {
            let argument = self.arguments[index].to_string_lossy();
            if argument == "--config" {
                index += 2;
            } else if argument.starts_with("--config=") {
                index += 1;
            } else {
                global.push(self.arguments[index].clone());
                index += 1;
            }
        }
        global.extend(self.configs());
        Ok(Inner {
            global,
            command,
            options,
        })
    }

    fn cargo_options(&self) -> Vec<&OsString> {
        let mut options = self
            .alias_options
            .iter()
            .take_while(|argument| *argument != "--")
            .collect::<Vec<_>>();
        options.extend(
            self.arguments
                .iter()
                .enumerate()
                .skip(1)
                .take_while(|(_, argument)| *argument != "--")
                .filter_map(|(index, argument)| (index != self.command).then_some(argument)),
        );
        options
    }
}

fn command_index(arguments: &[OsString]) -> Result<usize, Box<dyn Error>> {
    let mut index = 1;
    while index < arguments.len() {
        let argument = arguments[index].to_string_lossy();
        if argument.starts_with('+') || global_flag(&argument) {
            index += 1;
        } else if global_value(&argument) {
            index += usize::from(!argument.contains('=')) + 1;
        } else if argument.starts_with('-') {
            return Err(format!("unsupported Cargo global option `{argument}`").into());
        } else {
            return Ok(index);
        }
    }
    Err("Cargo subcommand is unavailable".into())
}

fn global_flag(argument: &str) -> bool {
    matches!(
        argument,
        "--verbose" | "-q" | "--quiet" | "--locked" | "--offline" | "--frozen"
    ) || argument.strip_prefix('-').is_some_and(|verbosity| {
        !verbosity.is_empty() && verbosity.chars().all(|character| character == 'v')
    })
}

fn global_value(argument: &str) -> bool {
    matches!(argument, "--color" | "--config" | "-Z" | "-C")
        || argument.starts_with("--color=")
        || argument.starts_with("--config=")
        || argument.starts_with("-Z") && argument.len() > 2
        || argument.starts_with("-C") && argument.len() > 2
}

fn resolve(
    token: &str,
    global: &[OsString],
    cwd: &Path,
    aliases: &mut Vec<String>,
) -> Result<(Operation, Vec<OsString>), Box<dyn Error>> {
    if let Some(operation) = operation(token) {
        return Ok((operation, Vec::new()));
    }
    if aliases.iter().any(|alias| alias == token) {
        return Err(format!("Cargo alias recursion: {} -> {token}", aliases.join(" -> ")).into());
    }
    aliases.push(token.to_owned());
    let cargo = env::var_os("CARGO").ok_or("CARGO is unavailable")?;
    let output = Command::new(cargo)
        .current_dir(cwd)
        .args(global)
        .arg("help")
        .arg(token)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "could not resolve Cargo alias `{token}`: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let help = String::from_utf8(output.stdout)?;
    if let Some(alias) = help.lines().find_map(|line| {
        line.strip_prefix(&format!("`{token}` is aliased to `"))?
            .strip_suffix('`')
    }) {
        let mut options = alias.split_whitespace();
        let command = options.next().ok_or("Cargo alias has no subcommand")?;
        if command.starts_with('!') {
            return Err("shell Cargo aliases are not artifact commands".into());
        }
        let own = options.map(OsString::from).collect::<Vec<_>>();
        let (operation, mut options) = resolve(command, global, cwd, aliases)?;
        options.extend(own);
        return Ok((operation, options));
    }
    let operation = help
        .lines()
        .find_map(|line| {
            line.strip_prefix("CARGO-")?
                .split_once('(')
                .map(|(name, _)| name.to_ascii_lowercase())
        })
        .and_then(|name| operation(&name))
        .ok_or_else(|| format!("Cargo alias `{token}` has no supported subcommand"))?;
    Ok((operation, Vec::new()))
}

fn operation(name: &str) -> Option<Operation> {
    match name {
        "build" | "b" => Some(Operation::Build),
        "check" | "c" => Some(Operation::Check),
        "clippy" => Some(Operation::Clippy),
        "test" | "t" => Some(Operation::Test),
        "bench" => Some(Operation::Bench),
        "doc" | "d" => Some(Operation::Doc),
        "run" | "r" => Some(Operation::Run),
        "install" => Some(Operation::Install),
        "package" => Some(Operation::Package),
        "publish" => Some(Operation::Publish),
        _ => None,
    }
}

fn selected_options<'a>(
    mut arguments: impl Iterator<Item = &'a OsString>,
    name: &str,
) -> Vec<OsString> {
    let mut options = Vec::new();
    while let Some(argument) = arguments.next() {
        if argument == name {
            if let Some(value) = arguments.next() {
                options.extend([argument.clone(), value.clone()]);
            }
        } else if argument.to_string_lossy().starts_with(&format!("{name}=")) {
            options.push(argument.clone());
        }
    }
    options
}

fn absolute_config(option: OsString, cwd: &Path) -> OsString {
    if option == "--config" {
        return option;
    }
    if let Some(value) = option
        .to_str()
        .and_then(|option| option.strip_prefix("--config="))
    {
        let value = absolute_config(OsString::from(value), cwd);
        let mut option = OsString::from("--config=");
        option.push(value);
        return option;
    }
    if option.to_string_lossy().contains('=') || Path::new(&option).is_absolute() {
        option
    } else {
        cwd.join(option).into_os_string()
    }
}

fn removed_option(operation: Operation, argument: &str) -> Option<bool> {
    if matches!(
        argument,
        "--manifest-path" | "-m" | "--target-dir" | "--package" | "-p" | "--exclude"
    ) {
        return Some(true);
    }
    if argument.starts_with("--manifest-path=")
        || argument.starts_with("--target-dir=")
        || argument.starts_with("--package=")
        || argument.starts_with("--exclude=")
        || argument.starts_with("-p") && argument.len() > 2
        || argument.starts_with("-m") && argument.len() > 2
    {
        return Some(false);
    }
    if matches!(argument, "--workspace" | "--all") {
        return Some(false);
    }
    let (flags, values): (&[&str], &[&str]) = match operation {
        Operation::Install => (
            &[
                "--list",
                "-f",
                "--force",
                "--no-track",
                "--debug",
                "-n",
                "--dry-run",
            ],
            &[
                "--root",
                "--path",
                "--git",
                "--branch",
                "--tag",
                "--rev",
                "--version",
                "--registry",
                "--index",
            ],
        ),
        Operation::Package => (
            &[
                "-l",
                "--list",
                "--allow-dirty",
                "--no-verify",
                "--no-metadata",
                "--exclude-lockfile",
            ],
            &["--registry", "--index"],
        ),
        Operation::Publish => (
            &["--allow-dirty", "--no-verify", "-n", "--dry-run"],
            &["--token", "--registry", "--index"],
        ),
        _ => return None,
    };
    if flags.contains(&argument) {
        return Some(false);
    }
    if values.contains(&argument) {
        return Some(true);
    }
    values
        .iter()
        .any(|option| argument.starts_with(&format!("{option}=")))
        .then_some(false)
}

fn previous_may_take_value(operation: Operation, arguments: &[OsString], index: usize) -> bool {
    index > 0
        && arguments[index - 1].to_str().is_some_and(|argument| {
            argument.starts_with('-')
                && !argument.contains('=')
                && removed_option(operation, argument) != Some(false)
        })
}
