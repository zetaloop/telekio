use std::{
    env,
    error::Error,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub(crate) fn build(
    out: &Path,
    records: &Path,
    mappings: &[(String, String)],
    windows: bool,
) -> Result<PathBuf, Box<dyn Error>> {
    let linker = env::var_os("RUSTC_LINKER").unwrap_or_else(|| {
        if windows {
            "link.exe".into()
        } else {
            "cc".into()
        }
    });
    let source = out.join("linker.rs");
    let executable = out.join(if cfg!(windows) {
        "linker.exe"
    } else {
        "linker"
    });
    let mappings = mappings
        .iter()
        .map(|(name, proxy)| format!("({name:?}, {proxy:?})"))
        .collect::<Vec<_>>()
        .join(",");
    fs::write(
        &source,
        format!(
            r###"use std::{{env, ffi::{{OsStr, OsString}}, fs, io::Write, path::Path, process::{{self, Command}}}};

const LINKER: &str = {linker:?};
const RECORDS: &str = {records:?};
const MAPPINGS: &[(&str, &str)] = &[{mappings}];
const WINDOWS_LINKER: bool = {windows};

fn main() {{
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    let output = Command::new(LINKER).args(&arguments).output().unwrap();
    if !output.status.success() {{
        std::io::stdout().write_all(&output.stdout).unwrap();
        std::io::stderr().write_all(&output.stderr).unwrap();
        process::exit(output.status.code().unwrap_or(1));
    }}
    let Some(crate_name) = env::var("CARGO_CRATE_NAME").ok() else {{ return }};
    let Some((_, proxy)) = MAPPINGS.iter().find(|(name, _)| *name == crate_name) else {{ return }};
    let arguments = response_arguments(&arguments);
    let directory = Path::new(RECORDS).join(format!("{{proxy}}.inputs"));
    fs::create_dir_all(&directory).unwrap();
    let mut recorded = Vec::new();
    let mut skip = false;
    for (index, argument) in arguments.into_iter().enumerate() {{
        if skip {{
            skip = false;
            continue;
        }}
        if output_argument(&argument) {{
            skip = !WINDOWS_LINKER && argument == "-o";
            continue;
        }}
        let path = Path::new(&argument);
        if linker_input(path) {{
            recorded.push(persist_input(path, &directory, index).into_os_string());
        }} else {{
            recorded.push(argument);
        }}
    }}
    write_record(&Path::new(RECORDS).join(format!("{{proxy}}.record")), &recorded);
}}

fn linker_input(path: &Path) -> bool {{
    path.is_file()
        && path
            .extension()
            .is_some_and(|extension| matches!(extension.to_str(), Some("o" | "obj" | "rlib" | "lib" | "a")))
}}

fn persist_input(path: &Path, directory: &Path, index: usize) -> std::path::PathBuf {{
    let extension = path.extension().and_then(OsStr::to_str);
    if !matches!(extension, Some("o" | "obj")) && temporary_path(path) {{
        let stable = path
            .parent()
            .and_then(Path::parent)
            .expect("temporary linker input has no stable parent")
            .join(path.file_name().expect("linker input has no name"));
        if stable.is_file() {{
            return stable;
        }}
    }}
    if matches!(extension, Some("o" | "obj")) || temporary_path(path) {{
        let destination = directory.join(format!("{{index}}-{{}}", path.file_name().unwrap().to_string_lossy()));
        if destination.is_file() {{
            fs::remove_file(&destination).unwrap();
        }}
        fs::hard_link(path, &destination).unwrap();
        destination
    }} else {{
        path.to_owned()
    }}
}}

fn output_argument(argument: &OsStr) -> bool {{
    let argument = argument.to_string_lossy();
    if WINDOWS_LINKER {{
        let upper = argument.to_ascii_uppercase();
        ["/OUT:", "/PDB:", "/IMPLIB:", "/ILK:", "/NATVIS:"]
            .iter()
            .any(|prefix| upper.starts_with(prefix))
            || upper.strip_prefix("/DEF:").is_some_and(|path| temporary_path(Path::new(path)))
    }} else {{
        argument == "-o"
            || argument.starts_with("--output=")
            || argument.starts_with("-Wl,-o,")
            || argument.starts_with("-Wl,--out-implib,")
            || argument.starts_with("-Wl,-Map,")
            || argument.starts_with("-Wl,--version-script=")
            || argument.starts_with("-Wl,--dynamic-list=")
    }}
}}

fn temporary_path(path: &Path) -> bool {{
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name.to_string_lossy().to_ascii_lowercase().starts_with("rustc"))
}}

fn response_arguments(arguments: &[OsString]) -> Vec<OsString> {{
    let Some(response) = arguments.first().and_then(|argument| argument.to_str()?.strip_prefix('@')) else {{ return arguments.to_owned() }};
    if WINDOWS_LINKER {{
        let bytes = fs::read(response).unwrap();
        let bytes = if bytes.starts_with(&[0xff, 0xfe]) {{ &bytes[2..] }} else {{ &bytes }};
        let words = bytes.chunks_exact(2).map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]])).collect::<Vec<_>>();
        String::from_utf16(&words).unwrap().lines().map(|line| OsString::from(line.trim_matches('"'))).collect()
    }} else {{
        fs::read_to_string(response).unwrap().lines().map(|line| OsString::from(line.trim_matches('"'))).collect()
    }}
}}

fn write_record(path: &Path, arguments: &[OsString]) {{
    let mut record = Vec::new();
    for argument in arguments {{
        let bytes = argument.as_encoded_bytes();
        record.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        record.extend_from_slice(bytes);
    }}
    fs::write(path, record).unwrap();
}}
"###,
            linker = linker.to_string_lossy(),
            records = records.display(),
        ),
    )?;
    let rustc = env::var_os("RUSTC").ok_or("RUSTC is unavailable")?;
    let status = Command::new(rustc)
        .arg("--edition")
        .arg("2024")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .status()?;
    if status.success() {
        Ok(executable)
    } else {
        Err(format!("linker helper compilation failed with {status}").into())
    }
}

pub(crate) fn read_record(path: &Path) -> Result<Vec<OsString>, Box<dyn Error>> {
    let record = fs::read(path)?;
    let mut position = 0;
    let mut arguments = Vec::new();
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
        // The record is written and read on the same host using OsStr's encoded bytes.
        arguments.push(unsafe { OsString::from_encoded_bytes_unchecked(bytes.to_vec()) });
        position += length;
    }
    if arguments.is_empty() {
        return Err(format!("{} recorded no linker inputs", path.display()).into());
    }
    Ok(arguments)
}
