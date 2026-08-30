use std::{
    env,
    error::Error,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct Spec {
    duplicate: Vec<&'static str>,
}

pub(crate) struct Mapping {
    pub proxy: String,
    pub entry: String,
}

pub(crate) struct Record {
    pub arguments: Vec<OsString>,
    pub spec: Spec,
}

impl Spec {
    pub fn replay(&self, arguments: &[OsString]) -> Vec<OsString> {
        let mut arguments = arguments.to_vec();
        arguments.extend(self.duplicate.iter().map(OsString::from));
        arguments
    }
}

pub(crate) fn archive(records: &Path, entry: &str) -> PathBuf {
    records.join(format!("{entry}.telekio"))
}

pub(crate) fn read_record(path: &Path, archive: PathBuf) -> Result<Record, Box<dyn Error>> {
    let record = fs::read(path)?;
    let mut position = 0;
    let mut command = Vec::new();
    while position < record.len() {
        let length = u64::from_le_bytes(
            record
                .get(position..position + 8)
                .ok_or("linker record ended before its argument length")?
                .try_into()?,
        ) as usize;
        position += 8;
        let bytes = record
            .get(position..position + length)
            .ok_or("linker record ended inside an argument")?;
        // The generated proxy and this reader use the same host and Rust toolchain.
        command.push(unsafe { OsString::from_encoded_bytes_unchecked(bytes.to_vec()) });
        position += length;
    }
    let program = command
        .first()
        .cloned()
        .ok_or("rustc recorded an empty linker command")?;
    let arguments = command.into_iter().skip(1).collect::<Vec<_>>();

    let msvc_output = arguments.iter().any(|argument| {
        argument
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("/OUT:")
    });
    let linker = Path::new(&program)
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let uses_cc = arguments
        .iter()
        .any(|argument| argument.to_string_lossy().starts_with("-Wl,"))
        || linker.ends_with("cc")
        || linker.contains("gcc")
        || linker.contains("clang");
    let apple = env::var("CARGO_CFG_TARGET_VENDOR").as_deref() == Ok("apple");
    let duplicate = if msvc_output {
        vec!["/FORCE:MULTIPLE"]
    } else if apple {
        Vec::new()
    } else if uses_cc {
        vec!["-Wl,--allow-multiple-definition"]
    } else {
        vec!["--allow-multiple-definition"]
    };

    Ok(Record {
        arguments: filter_arguments(arguments, archive, msvc_output)?,
        spec: Spec { duplicate },
    })
}

fn filter_arguments(
    arguments: Vec<OsString>,
    archive: PathBuf,
    msvc_output: bool,
) -> Result<Vec<OsString>, Box<dyn Error>> {
    let arguments = remove_outputs(arguments, msvc_output);
    let mut recorded = Vec::new();
    let mut first_file = None;
    let mut static_file = None;
    let mut static_mode = msvc_output;
    for argument in arguments {
        if linker_input(Path::new(&argument)) {
            first_file.get_or_insert(recorded.len());
            if static_mode {
                static_file.get_or_insert(recorded.len());
            }
        } else {
            static_mode |= argument == "-Wl,-Bstatic";
            static_mode &= argument != "-Wl,-Bdynamic";
            recorded.push(argument);
        }
    }
    let position = static_file
        .or(first_file)
        .ok_or("linker invocation contained no file inputs")?;
    recorded.insert(position, archive.into_os_string());
    Ok(recorded)
}

fn linker_input(path: &Path) -> bool {
    let located = path.is_absolute()
        || path
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty());
    located
        && path.extension().is_some_and(|extension| {
            matches!(
                extension.to_str(),
                Some("o" | "obj" | "rlib" | "lib" | "a" | "telekio")
            )
        })
}

fn remove_outputs(mut arguments: Vec<OsString>, msvc_output: bool) -> Vec<OsString> {
    if arguments
        .first()
        .is_some_and(|argument| argument == "-flavor")
        && arguments.len() > 1
    {
        arguments.drain(..2);
    }
    let mut filtered = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] == "-Xlinker"
            && let Some(argument) = arguments.get(index + 1)
        {
            let next = (arguments.get(index + 2) == Some(&OsString::from("-Xlinker")))
                .then(|| arguments.get(index + 3))
                .flatten();
            if output_pair(argument, next) {
                index += 4;
                continue;
            }
            if output_argument(argument, msvc_output) {
                index += 2;
                continue;
            }
            filtered.extend_from_slice(&arguments[index..index + 2]);
            index += 2;
            continue;
        }
        if output_pair(&arguments[index], arguments.get(index + 1)) {
            index += 2;
            continue;
        }
        if output_argument(&arguments[index], msvc_output) {
            index += 1;
            continue;
        }
        if arguments[index]
            .to_str()
            .is_some_and(|argument| argument.starts_with("-Wl,"))
        {
            if let Some(argument) = filter_driver_argument(&arguments[index]) {
                filtered.push(argument);
            }
        } else {
            filtered.push(arguments[index].clone());
        }
        index += 1;
    }
    filtered
}

fn output_pair(argument: &OsStr, next: Option<&OsString>) -> bool {
    let Some(next) = next else { return false };
    if argument == "-o" || argument == "--out-implib" {
        return true;
    }
    matches!(
        argument.to_str(),
        Some("-exported_symbols_list" | "--dynamic-list" | "-M" | "-Map")
    ) && temporary_path(Path::new(next))
}

fn output_argument(argument: &OsStr, msvc_output: bool) -> bool {
    let argument = argument.to_string_lossy();
    let upper = argument.to_ascii_uppercase();
    if msvc_output
        && (["/OUT:", "/PDB:", "/IMPLIB:", "/ILK:"]
            .iter()
            .any(|prefix| upper.starts_with(prefix))
            || upper
                .strip_prefix("/DEF:")
                .is_some_and(|path| temporary_path(Path::new(path)))
            || upper.strip_prefix("/NATVIS:").is_some_and(rustc_natvis))
    {
        return true;
    }
    argument.starts_with("--output=")
        || argument.starts_with("--out-implib=")
        || temporary_linker_output(&argument, "-Map=")
        || temporary_linker_output(&argument, "--version-script=")
        || temporary_linker_output(&argument, "--dynamic-list=")
}

fn filter_driver_argument(argument: &OsStr) -> Option<OsString> {
    let argument = argument.to_str()?;
    let parts = argument
        .strip_prefix("-Wl,")?
        .split(',')
        .collect::<Vec<_>>();
    let mut filtered = Vec::new();
    let mut index = 0;
    while index < parts.len() {
        let part = parts[index];
        let next = parts.get(index + 1).copied();
        if matches!(part, "-o" | "--out-implib") && next.is_some() {
            index += 2;
            continue;
        }
        if matches!(
            part,
            "-exported_symbols_list" | "--dynamic-list" | "-M" | "-Map"
        ) && next.is_some_and(|path| temporary_path(Path::new(path)))
        {
            index += 2;
            continue;
        }
        if temporary_linker_output(part, "-Map=")
            || temporary_linker_output(part, "--version-script=")
            || temporary_linker_output(part, "--dynamic-list=")
        {
            index += 1;
            continue;
        }
        filtered.push(part);
        index += 1;
    }
    (!filtered.is_empty()).then(|| format!("-Wl,{}", filtered.join(",")).into())
}

fn temporary_linker_output(argument: &str, prefix: &str) -> bool {
    argument
        .strip_prefix(prefix)
        .is_some_and(|path| temporary_path(Path::new(path)))
}

fn temporary_path(path: &Path) -> bool {
    path.parent().and_then(Path::file_name).is_some_and(|name| {
        name.to_string_lossy()
            .to_ascii_lowercase()
            .starts_with("rustc")
    })
}

fn rustc_natvis(path: &str) -> bool {
    let path = Path::new(path);
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|parent| parent == "etc")
        && path
            .ancestors()
            .any(|ancestor| ancestor.file_name() == Some("rustlib".as_ref()))
}
